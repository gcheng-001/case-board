//! 修订批注版 docx 生成(2026-06-17 · 合同审查 P2 + P3)。
//!
//! 在**原始 docx 上**落痕(保留原排版),不重新生成:
//! - **P2 整段批注**(鲁棒底线):为风险所在段落加 Word 批注(`w:commentRangeStart/End` +
//!   `w:commentReference` + `word/comments.xml`)。无需拆 run。
//! - **P3 行内修订痕迹**:对 `action=revise` 且 `anchor_text` 能在段落内精确定位、且覆盖的 run
//!   全是纯文本时,做 run-split,把原文包 `w:del`(`w:delText`)、推荐措辞包 `w:ins`,并在其上挂一条
//!   解释批注。
//! - **降级链**(advisor 定):定位不到 / 覆盖到复杂 run(图片/域)/ run 重叠 → **降级为整段批注**;
//!   段落缺失(`paragraph_index=null`)→ 收进 `skipped`,只在审查意见书提示,**绝不静默丢弃**(坑 #8)。
//!
//! 段落 / run 切分复用 [`super::parse`] 的唯一权威实现,保证 `paragraph_index` 与审查阶段零漂移。

use std::io::{Cursor, Read, Write};

use crate::contract_review::analyze::{ContractReviewResult, ReviewRisk};
use crate::contract_review::parse;

const COMMENTS_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml";
const COMMENTS_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";

/// docx 内全部 part(保序):`(部件路径, 字节内容)`。
type DocxParts = Vec<(String, Vec<u8>)>;

/// 生成结果。
pub struct RedlineOutcome {
    pub docx: Vec<u8>,
    /// 落了行内修订痕迹的条数
    pub applied_inline: usize,
    /// 落了整段批注的条数
    pub applied_comment: usize,
    /// 未能落入 Word 正文、只在意见书提示的条目(给 report 的 skipped)
    pub skipped: Vec<String>,
}

/// 一条 document.xml 上的字节级编辑(`start==end` 即纯插入)。
struct Edit {
    start: usize,
    end: usize,
    replacement: String,
}

/// 一条批注内容。
struct CommentEntry {
    id: usize,
    text: String,
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// 批注正文(分级 / 风险点 / 后果 / 建议 / 推荐措辞)。
fn comment_text(risk: &ReviewRisk) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("【{}】{}", risk.norm_level(), risk.title.trim()));
    if !risk.clause_ref.trim().is_empty() {
        lines.push(format!("条款位置:{}", risk.clause_ref.trim()));
    }
    if !risk.consequence.trim().is_empty() {
        lines.push(format!("风险后果:{}", risk.consequence.trim()));
    }
    if !risk.suggestion.trim().is_empty() {
        lines.push(format!("整改建议:{}", risk.suggestion.trim()));
    }
    if !risk.recommended_text.trim().is_empty() {
        lines.push(format!("推荐措辞:{}", risk.recommended_text.trim()));
    }
    if !risk.basis.trim().is_empty() {
        lines.push(format!("法律依据:{}", risk.basis.trim()));
    }
    lines.join("\n")
}

/// run 引用样式 + commentReference run(放在 commentRangeEnd 后)。
fn comment_reference_run(id: usize) -> String {
    format!(
        "<w:r><w:rPr><w:rStyle w:val=\"CommentReference\"/></w:rPr><w:commentReference w:id=\"{}\"/></w:r>",
        id
    )
}

/// 纯文本 run 片段(沿用给定 rPr,xml:space preserve 防首尾空白被吞)。
fn text_run(rpr_xml: &str, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    format!(
        "<w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
        rpr_xml,
        xml_escape(text)
    )
}

/// 主入口:原 docx + 审查结果 → 修订批注版 docx 字节。
pub fn build_redlined_docx(
    src_path: &str,
    result: &ContractReviewResult,
    author: &str,
) -> Result<RedlineOutcome, String> {
    // 1. 读出原 docx 全部 part(保序),拿 document.xml。
    let (mut parts, doc_xml) = read_docx_parts(src_path)?;
    let date = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let author_esc = xml_escape(author);

    // 2. 段落切分(唯一权威)。
    let para_spans = parse::find_paragraph_spans(&doc_xml);

    let mut edits: Vec<Edit> = Vec::new();
    let mut comments: Vec<CommentEntry> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut applied_inline = 0usize;
    let mut applied_comment = 0usize;
    let mut comment_id = 0usize;
    let mut rev_id = 1000usize; // 修订 id 自有空间,避开批注 id
                                // 每段已被行内修订占用的 run 字节区间(避免同段多条修订重叠)。
    let mut occupied: std::collections::HashMap<usize, Vec<(usize, usize)>> =
        std::collections::HashMap::new();

    // 按段落 + 段内位置稳定排序(让落痕顺序自然)。
    let mut risks: Vec<&ReviewRisk> = result.risks.iter().collect();
    risks.sort_by_key(|r| r.paragraph_index.unwrap_or(usize::MAX));

    for risk in risks {
        let title = risk.title.trim();
        let pidx = match risk.paragraph_index {
            Some(i) if i < para_spans.len() => i,
            _ => {
                // 缺失条款 / 无法定位段落 → 只进意见书
                skipped.push(format!("[{}] {}", risk.norm_level(), title));
                continue;
            }
        };
        let (ps, pe) = para_spans[pidx];
        let para_xml = &doc_xml[ps..pe];

        // 自闭合空段无内容,不批注
        if !para_xml.ends_with("</w:p>") {
            skipped.push(format!(
                "[{}] {}(段落为空,无法落痕)",
                risk.norm_level(),
                title
            ));
            continue;
        }

        // 尝试行内修订
        let mut did_inline = false;
        if risk.wants_revise() {
            if let Some((rstart, rend, replacement)) =
                try_build_inline(para_xml, risk, &author_esc, &date, &mut rev_id, comment_id)
            {
                let used = occupied.entry(pidx).or_default();
                let overlap = used.iter().any(|&(a, b)| rstart < b && a < rend);
                if !overlap {
                    used.push((rstart, rend));
                    edits.push(Edit {
                        start: ps + rstart,
                        end: ps + rend,
                        replacement,
                    });
                    comments.push(CommentEntry {
                        id: comment_id,
                        text: comment_text(risk),
                    });
                    comment_id += 1;
                    applied_inline += 1;
                    did_inline = true;
                }
            }
        }

        // 行内没成 → 整段批注
        if !did_inline {
            let (a, b) = whole_paragraph_comment_points(para_xml);
            edits.push(Edit {
                start: ps + a,
                end: ps + a,
                replacement: format!("<w:commentRangeStart w:id=\"{}\"/>", comment_id),
            });
            edits.push(Edit {
                start: ps + b,
                end: ps + b,
                replacement: format!(
                    "<w:commentRangeEnd w:id=\"{}\"/>{}",
                    comment_id,
                    comment_reference_run(comment_id)
                ),
            });
            comments.push(CommentEntry {
                id: comment_id,
                text: comment_text(risk),
            });
            comment_id += 1;
            applied_comment += 1;
        }
    }

    // 3. 应用编辑(按起点降序,避免偏移漂移)。
    let new_doc_xml = apply_edits(&doc_xml, edits);

    // 4. 没有任何批注 → 仍返回(等于原文副本),让上层决定提示。
    if comments.is_empty() {
        parts_set(&mut parts, "word/document.xml", new_doc_xml.into_bytes());
        let docx = write_docx_parts(parts)?;
        return Ok(RedlineOutcome {
            docx,
            applied_inline,
            applied_comment,
            skipped,
        });
    }

    // 5. 生成 comments.xml + 注册 content_types / rels。
    let comments_xml = build_comments_xml(&comments, &author_esc, &date);
    parts_set(&mut parts, "word/document.xml", new_doc_xml.into_bytes());
    parts_set(&mut parts, "word/comments.xml", comments_xml.into_bytes());
    register_comments_part(&mut parts)?;

    let docx = write_docx_parts(parts)?;
    Ok(RedlineOutcome {
        docx,
        applied_inline,
        applied_comment,
        skipped,
    })
}

/// 整段批注的两个插入点(相对段落 XML):A=pPr 之后(或 `<w:p>` 开标签后),B=`</w:p>` 前。
fn whole_paragraph_comment_points(para_xml: &str) -> (usize, usize) {
    // A:pPr 之后;无 pPr 则开标签 '>' 之后
    let a = match parse::next_element_span(para_xml, "w:pPr", 0) {
        Some((_, e)) => e,
        None => para_xml.find('>').map(|i| i + 1).unwrap_or(0),
    };
    // B:</w:p> 之前
    let b = para_xml.len() - "</w:p>".len();
    (a, b)
}

/// 尝试为一条 revise 风险构造行内修订替换。返回 (段内起, 段内止, 替换 XML),不可行则 None。
fn try_build_inline(
    para_xml: &str,
    risk: &ReviewRisk,
    author_esc: &str,
    date: &str,
    rev_id: &mut usize,
    cid: usize,
) -> Option<(usize, usize, String)> {
    let anchor = risk.anchor_text.trim();
    if anchor.is_empty() {
        return None;
    }
    let pieces = parse::find_run_pieces(para_xml);
    if pieces.is_empty() {
        return None;
    }
    // 拼接段落文本,定位 anchor 字符区间
    let full: String = pieces.iter().map(|p| p.text.as_str()).collect();
    let c0 = full.find(anchor)?;
    let c1 = c0 + anchor.len();

    // 找覆盖 [c0,c1) 的 run 索引区间 [i..=j]
    let mut acc = 0usize;
    let mut i = None;
    let mut j = None;
    let mut starts: Vec<usize> = Vec::with_capacity(pieces.len());
    for (k, p) in pieces.iter().enumerate() {
        starts.push(acc);
        let next = acc + p.text.len();
        if i.is_none() && c0 < next {
            i = Some(k);
        }
        if c1 <= next {
            j = Some(k);
            break;
        }
        acc = next;
    }
    let (i, j) = (i?, j?);
    // 覆盖到的 run 必须全是纯文本,否则不敢拆 → 降级
    if !pieces[i..=j].iter().all(|p| p.simple) {
        return None;
    }

    // 受影响 run 区间的拼接文本与边界
    let base = starts[i];
    let affected: String = pieces[i..=j].iter().map(|p| p.text.as_str()).collect();
    let a0 = c0 - base;
    let a1 = c1 - base;
    if a0 > affected.len() || a1 > affected.len() || a0 > a1 {
        return None;
    }
    let prefix = &affected[..a0];
    let suffix = &affected[a1..];

    // 受影响区间含 <w:br/>/<w:tab/>(被 normalize 成 \n/\t):若直接重建成 <w:t> 文本,Word 会把
    // 换行/制表塌成空格,等于静默改了**保留下来**的前后缀文本 → 降级整段批注,绝不破坏原文。
    if anchor.contains(['\n', '\t'])
        || prefix.contains(['\n', '\t'])
        || suffix.contains(['\n', '\t'])
    {
        return None;
    }

    let rpr_i = pieces[i].rpr_xml.as_str();
    let rpr_j = pieces[j].rpr_xml.as_str();

    let del_id = *rev_id;
    let ins_id = *rev_id + 1;
    *rev_id += 2;

    let mut out = String::new();
    out.push_str(&text_run(rpr_i, prefix));
    out.push_str(&format!("<w:commentRangeStart w:id=\"{}\"/>", cid));
    // 删除原文
    out.push_str(&format!(
        "<w:del w:id=\"{}\" w:author=\"{}\" w:date=\"{}\"><w:r>{}<w:delText xml:space=\"preserve\">{}</w:delText></w:r></w:del>",
        del_id, author_esc, date, rpr_i, xml_escape(anchor)
    ));
    // 插入推荐措辞(为空则纯删除)
    let recommended = risk.recommended_text.trim();
    if !recommended.is_empty() {
        out.push_str(&format!(
            "<w:ins w:id=\"{}\" w:author=\"{}\" w:date=\"{}\"><w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:ins>",
            ins_id, author_esc, date, rpr_i, xml_escape(recommended)
        ));
    }
    out.push_str(&format!("<w:commentRangeEnd w:id=\"{}\"/>", cid));
    out.push_str(&comment_reference_run(cid));
    out.push_str(&text_run(rpr_j, suffix));

    Some((pieces[i].span.0, pieces[j].span.1, out))
}

/// 应用编辑:按起点降序 splice,避免偏移漂移。
fn apply_edits(doc_xml: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by_key(|e| std::cmp::Reverse(e.start));
    let mut s = doc_xml.to_string();
    for e in edits {
        if e.start <= e.end && e.end <= s.len() {
            s.replace_range(e.start..e.end, &e.replacement);
        }
    }
    s
}

/// 生成 word/comments.xml。
fn build_comments_xml(comments: &[CommentEntry], author_esc: &str, date: &str) -> String {
    let mut body = String::new();
    for c in comments {
        // 多行批注 → 多个 <w:p>
        let paras: String = c
            .text
            .split('\n')
            .map(|line| {
                format!(
                    "<w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
                    xml_escape(line)
                )
            })
            .collect();
        body.push_str(&format!(
            "<w:comment w:id=\"{}\" w:author=\"{}\" w:date=\"{}\" w:initials=\"\">{}</w:comment>",
            c.id, author_esc, date, paras
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:comments xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">{}</w:comments>",
        body
    )
}

/// 读 docx 全部 part(保序),并返回 document.xml 字符串。
fn read_docx_parts(path: &str) -> Result<(DocxParts, String), String> {
    let f = std::fs::File::open(path).map_err(|e| format!("打开 .docx 失败: {}", e))?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("读 .docx zip 失败: {}", e))?;
    let mut parts: Vec<(String, Vec<u8>)> = Vec::with_capacity(zip.len());
    let mut doc_xml: Option<String> = None;
    for idx in 0..zip.len() {
        let mut entry = zip
            .by_index(idx)
            .map_err(|e| format!("读 zip 条目失败: {}", e))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| format!("读 zip 条目 {} 失败: {}", name, e))?;
        if name == "word/document.xml" {
            doc_xml = Some(
                String::from_utf8(bytes.clone())
                    .map_err(|e| format!("document.xml 非 UTF-8: {}", e))?,
            );
        }
        parts.push((name, bytes));
    }
    let doc_xml = doc_xml.ok_or_else(|| ".docx 内找不到 word/document.xml".to_string())?;
    Ok((parts, doc_xml))
}

/// 覆盖 / 新增一个 part。
fn parts_set(parts: &mut DocxParts, name: &str, bytes: Vec<u8>) {
    if let Some(slot) = parts.iter_mut().find(|(n, _)| n == name) {
        slot.1 = bytes;
    } else {
        parts.push((name.to_string(), bytes));
    }
}

/// 把 comments.xml 注册进 [Content_Types].xml + word/_rels/document.xml.rels。
fn register_comments_part(parts: &mut DocxParts) -> Result<(), String> {
    // [Content_Types].xml:加 Override(已存在则跳过)
    {
        let ct = parts
            .iter_mut()
            .find(|(n, _)| n == "[Content_Types].xml")
            .ok_or_else(|| ".docx 缺 [Content_Types].xml".to_string())?;
        let mut s = String::from_utf8(ct.1.clone())
            .map_err(|e| format!("[Content_Types] 非 UTF-8: {}", e))?;
        if !s.contains("/word/comments.xml") {
            let ins = format!(
                "<Override PartName=\"/word/comments.xml\" ContentType=\"{}\"/>",
                COMMENTS_CONTENT_TYPE
            );
            s = s.replacen("</Types>", &format!("{}</Types>", ins), 1);
            ct.1 = s.into_bytes();
        }
    }

    // word/_rels/document.xml.rels:加 Relationship(已存在则跳过)
    {
        let rels_name = "word/_rels/document.xml.rels";
        if let Some(rels) = parts.iter_mut().find(|(n, _)| n == rels_name) {
            let mut s = String::from_utf8(rels.1.clone())
                .map_err(|e| format!("document.xml.rels 非 UTF-8: {}", e))?;
            if !s.contains("comments.xml") {
                let rid = fresh_rid(&s);
                let ins = format!(
                    "<Relationship Id=\"{}\" Type=\"{}\" Target=\"comments.xml\"/>",
                    rid, COMMENTS_REL_TYPE
                );
                s = s.replacen("</Relationships>", &format!("{}</Relationships>", ins), 1);
                rels.1 = s.into_bytes();
            }
        } else {
            // 没有 rels(极少见)→ 新建
            let body = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rIdCb1\" Type=\"{}\" Target=\"comments.xml\"/></Relationships>",
                COMMENTS_REL_TYPE
            );
            parts.push((rels_name.to_string(), body.into_bytes()));
        }
    }
    Ok(())
}

/// 生成一个不与现有 `rId{N}` 冲突的关系 id。
fn fresh_rid(rels_xml: &str) -> String {
    let mut max = 0usize;
    let mut from = 0usize;
    while let Some(rel) = rels_xml[from..].find("Id=\"rId") {
        let start = from + rel + "Id=\"rId".len();
        let num: String = rels_xml[start..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(n) = num.parse::<usize>() {
            max = max.max(n);
        }
        from = start;
    }
    format!("rId{}", max + 1)
}

/// 把全部 part 写回一个 docx zip。
fn write_docx_parts(parts: DocxParts) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in &parts {
            zip.start_file(name, opts)
                .map_err(|e| format!("写 zip 条目 {} 失败: {}", name, e))?;
            zip.write_all(bytes)
                .map_err(|e| format!("写 zip 内容 {} 失败: {}", name, e))?;
        }
        zip.finish().map_err(|e| format!("收尾 zip 失败: {}", e))?;
    }
    Ok(buf)
}
