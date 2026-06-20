//! 合同 .docx 单次解析(2026-06-17 · 合同审查)。
//!
//! **设计铁律:一次切分,两个消费方,零漂移。**
//! - 消费方 ①(审查):把每段正文按段落编号喂给 LLM。
//! - 消费方 ②(redline):按 `paragraph_index` 在**同一套段落切分**里定位 `<w:p>`,落批注 / 修订痕迹。
//!
//! 两侧必须看到**完全一致**的段落顺序与文本,否则 LLM 给的 `paragraph_index` / `anchor_text`
//! 在 redline 阶段对不上。为此:
//! - 段落切分唯一权威 = [`find_paragraph_spans`](自字符串扫描 `<w:p>`,**含自闭合 `<w:p/>` 空段**,
//!   index 连续);文本路径与 redline 路径都走它。
//! - **不 trim 段内文本、不跳过空段**(trim/跳过会移动偏移与 index,破坏 redline 定位)。
//! - run 级切分 = [`find_run_pieces`],给 redline 拿 run 的文本 / rPr / 是否纯文本 / 字节区间。
//! - normalization 跟 `docx_extract` 对齐:`xml_content()` 解码 XML entity,`<w:br/>` → `\n`,`<w:tab/>` → `\t`。

use std::fs::File;
use std::io::Read;

use quick_xml::events::Event;
use quick_xml::reader::Reader;

const MAX_DOCX_BYTES: u64 = 50 * 1024 * 1024;

/// 一个 `<w:r>` run 的文本(轻量,用于文本拼接路径)。
#[derive(Debug, Clone)]
pub struct RunText {
    pub text: String,
}

/// 一个 `<w:p>` 段落。`index` = 该段在文档里的顺序位置(从 0 起,含空段)。
#[derive(Debug, Clone)]
pub struct Paragraph {
    pub index: usize,
    pub runs: Vec<RunText>,
    /// 该段 `<w:p>...</w:p>`(或自闭合 `<w:p/>`)在 `raw_document_xml` 里的字节区间 `[start, end)`。
    pub span: (usize, usize),
}

impl Paragraph {
    /// 段落文本 = 各 run 文本顺序拼接(**不 trim**,保证 redline 偏移对齐)。
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }
}

/// run 级切片(给 redline run-split 用)。
#[derive(Debug, Clone)]
pub struct RunPiece {
    /// run 拼接文本(`<w:t>` + `<w:br/>`→\n + `<w:tab/>`→\t,不 trim)
    pub text: String,
    /// `<w:rPr>...</w:rPr>` 原文(含标签;无则空串)。run-split 重建子 run 时沿用,保格式。
    pub rpr_xml: String,
    /// run 是否「纯文本」(只含 rPr / t / br / tab)。含图片/域/公式等 → false,redline 不拆它。
    pub simple: bool,
    /// run `<w:r>...</w:r>` 在所属段落 XML 子串里的字节区间 `[start, end)`。
    pub span: (usize, usize),
}

/// 一份解析后的合同。
#[derive(Debug, Clone)]
pub struct ParsedContract {
    pub paragraphs: Vec<Paragraph>,
    /// 原始 `word/document.xml`,留给 redline 阶段直接 patch(避免二次解包)。
    pub raw_document_xml: String,
}

impl ParsedContract {
    /// 全文纯文本(段落间 `\n\n`,trim 每段)—— 仅用于体量估算 / 兜底显示,不用于 redline 定位。
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for p in &self.paragraphs {
            let t = p.text();
            let t = t.trim();
            if t.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(t);
        }
        out
    }

    /// 喂 LLM 的「带段落编号」文本。空段落跳过(不喂),但 `[P{index}]` 仍是原始 index,
    /// 这样 LLM 回的 index 能在 `paragraphs` 里直接索引到。
    pub fn numbered_text(&self) -> String {
        let mut out = String::new();
        for p in &self.paragraphs {
            let t = p.text();
            let t = t.trim();
            if t.is_empty() {
                continue;
            }
            out.push_str(&format!("[P{}] {}\n", p.index, t));
        }
        out
    }

    /// 非空段落数(给前端 / 日志看体量)。
    pub fn non_empty_count(&self) -> usize {
        self.paragraphs
            .iter()
            .filter(|p| !p.text().trim().is_empty())
            .count()
    }
}

/// 解析合同 .docx → `ParsedContract`。失败透传真错(坑 #8)。
pub fn parse_contract_docx(path: &str) -> Result<ParsedContract, String> {
    let p = std::path::Path::new(path);
    if !p.exists() {
        return Err(format!("文件不存在: {}", path));
    }
    let meta = std::fs::metadata(p).map_err(|e| format!("读元信息失败: {}", e))?;
    if meta.len() > MAX_DOCX_BYTES {
        return Err(format!(
            ".docx 过大({:.1} MB),超过 {} MB 上限",
            meta.len() as f64 / 1024.0 / 1024.0,
            MAX_DOCX_BYTES / 1024 / 1024
        ));
    }

    let doc_xml = read_document_xml(path)?;
    let paragraphs = parse_paragraphs(&doc_xml);
    if paragraphs.is_empty() {
        return Err("合同正文为空(未解析到任何段落)".to_string());
    }
    Ok(ParsedContract {
        paragraphs,
        raw_document_xml: doc_xml,
    })
}

/// 从 .docx zip 取 `word/document.xml` 原文。redline 阶段也用。
pub fn read_document_xml(path: &str) -> Result<String, String> {
    let f = File::open(path).map_err(|e| format!("打开 .docx 失败: {}", e))?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| {
        format!(
            "读 .docx zip 失败(可能不是 OOXML 格式,是旧 .doc 二进制?): {}",
            e
        )
    })?;
    let mut doc_xml = String::new();
    let mut entry = zip
        .by_name("word/document.xml")
        .map_err(|e| format!(".docx 内找不到 word/document.xml: {}", e))?;
    entry
        .read_to_string(&mut doc_xml)
        .map_err(|e| format!("读 word/document.xml 失败: {}", e))?;
    Ok(doc_xml)
}

/// 段落切分**唯一权威**:从 `document.xml` 扫出每个 `<w:p>...</w:p>` / `<w:p/>` 的字节区间,
/// index 连续(含空段)。redline 必须复用本函数定位段落。
pub fn parse_paragraphs(doc_xml: &str) -> Vec<Paragraph> {
    find_paragraph_spans(doc_xml)
        .into_iter()
        .enumerate()
        .map(|(index, span)| {
            let para_xml = &doc_xml[span.0..span.1];
            let runs = find_run_pieces(para_xml)
                .into_iter()
                .map(|rp| RunText { text: rp.text })
                .collect();
            Paragraph { index, runs, span }
        })
        .collect()
}

/// 扫出所有 `<w:p>` 段落的字节区间(配对或自闭合)。
pub fn find_paragraph_spans(doc_xml: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut from = 0usize;
    while let Some((start, end)) = next_element_span(doc_xml, "w:p", from) {
        spans.push((start, end));
        from = end;
    }
    spans
}

/// 在段落 XML 子串里扫出每个 `<w:r>` run 的切片(给 redline run-split)。
pub fn find_run_pieces(para_xml: &str) -> Vec<RunPiece> {
    let mut pieces = Vec::new();
    let mut from = 0usize;
    while let Some((start, end)) = next_element_span(para_xml, "w:r", from) {
        let run_xml = &para_xml[start..end];
        let (text, simple) = run_text_and_simple(run_xml);
        let rpr_xml = next_element_span(run_xml, "w:rPr", 0)
            .map(|(s, e)| run_xml[s..e].to_string())
            .unwrap_or_default();
        pieces.push(RunPiece {
            text,
            rpr_xml,
            simple,
            span: (start, end),
        });
        from = end;
    }
    pieces
}

/// 从一个 run 的 XML 抽:① 拼接文本(t/br/tab);② 是否纯文本 run。
fn run_text_and_simple(run_xml: &str) -> (String, bool) {
    let mut reader = Reader::from_str(run_xml);
    let mut buf = Vec::new();
    let mut text = String::new();
    let mut simple = true;
    let mut rpr_depth = 0i32;
    let mut in_t = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"rPr" => rpr_depth += 1,
                    _ if rpr_depth > 0 => {} // rPr 内的格式子元素,忽略
                    b"r" => {}               // run 外壳
                    b"t" => in_t = true,
                    // run 直接子级出现 t/br/tab 以外的元素(drawing/fldChar/instrText/object…)→ 非纯文本
                    _ => simple = false,
                }
            }
            Ok(Event::End(e)) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"rPr" => rpr_depth -= 1,
                    b"t" => in_t = false,
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let local = e.local_name();
                if rpr_depth > 0 {
                    // rPr 内自闭合格式元素,忽略
                } else {
                    match local.as_ref() {
                        b"br" => text.push('\n'),
                        b"tab" => text.push('\t'),
                        b"rPr" => {} // 自闭合空 rPr
                        b"t" => {}   // 自闭合空 t
                        _ => simple = false,
                    }
                }
            }
            Ok(Event::Text(e)) if in_t && rpr_depth == 0 => {
                if let Ok(raw) = e.xml_content() {
                    text.push_str(raw.as_ref());
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    (text, simple)
}

/// 从 `xml[from..]` 找下一个 `<{tag}>` 元素(配对或自闭合)的字节区间 `[start, end)`。
///
/// - 仅匹配 `<{tag}` 后紧跟 `>` / 空白 / `/` 的,**排除前缀撞名**(如找 `w:p` 不会误中 `w:pPr`)。
/// - 假设同名元素**不嵌套**(OOXML 里 `w:p`/`w:r` 都不嵌套同名),故配对找下一个 `</{tag}>` 即正确。
pub fn next_element_span(xml: &str, tag: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = xml.as_bytes();
    let open_pat = format!("<{}", tag);
    let close_pat = format!("</{}>", tag);
    let mut search = from;
    loop {
        let rel = xml.get(search..)?.find(&open_pat)?;
        let start = search + rel;
        let after = start + open_pat.len();
        // 后一个字符必须是 '>' / 空白 / '/',否则是前缀撞名(w:pPr)→ 继续找
        let next_ok = matches!(
            bytes.get(after).copied(),
            Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'/')
        );
        if !next_ok {
            search = after;
            continue;
        }
        // 找开始标签的 '>'
        let gt_rel = xml.get(after..)?.find('>')?;
        let open_end = after + gt_rel + 1; // 含 '>'
                                           // 自闭合? '>' 前一个字符是 '/'
        if open_end >= 2 && bytes[open_end - 2] == b'/' {
            return Some((start, open_end));
        }
        // 配对 </tag>
        let close_rel = xml.get(open_end..)?.find(&close_pat)?;
        let end = open_end + close_rel + close_pat.len();
        return Some((start, end));
    }
}
