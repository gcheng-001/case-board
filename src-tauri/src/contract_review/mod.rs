//! 合同审查模块(2026-06-17 · 非诉 tab「合同审查」功能)。
//!
//! 数据流:上传 .docx → `parse` 单次解析(段落编号文本 + run 结构)→ `analyze` LLM 审查
//! (结构化 JSON)→ `report` 出审查意见书 docx + `redline` 出修订批注版 docx(P2 整段批注 +
//! P3 行内修订痕迹 w:ins/w:del)。
//!
//! **Clean-room**:方法论参考杨卫薪律师 contract-copilot(CC BY-NC),prompt / schema / 引擎全自建,
//! 零照搬其知识 md 与 Python 代码。详见 `docs/提案-合同审查-2026-06-17.md`。

pub mod analyze;
pub mod parse;
pub mod redline;
pub mod report;

use serde::Serialize;

use analyze::{ContractReviewResult, Stance, Strictness};

/// 从 docx 路径推合同名(去扩展名)。
fn contract_name_from_path(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "合同".to_string())
}

/// 审查命令返回给前端的载荷。
#[derive(Debug, Serialize)]
pub struct ContractReviewResponse {
    /// 从文件名推出的合同名(前端展示 + 导出时回传)
    pub contract_name: String,
    /// 非空段落数(给前端看体量)
    pub paragraph_count: usize,
    /// 结构化审查结果(风险清单 + 结论)
    pub result: ContractReviewResult,
    /// 审查意见书 Markdown(前端预览用)
    pub opinion_md: String,
}

/// P1 主命令:审查一份合同 .docx,返回风险清单 + 结论 + 意见书 MD。
///
/// 不落库、不依赖 pool —— 纯工具形态(合同未必属于某个案件)。失败透传真错(坑 #8)。
#[tauri::command]
pub async fn review_contract_docx(
    docx_path: String,
    stance: String,
    strictness: String,
    contract_type_hint: String,
) -> Result<ContractReviewResponse, String> {
    let parsed = parse::parse_contract_docx(&docx_path)?;
    let numbered = parsed.numbered_text();
    if numbered.trim().is_empty() {
        return Err("合同正文为空,未解析到可审查的文字(可能是扫描件图片?请先 OCR)".to_string());
    }

    let settings = crate::settings::read_settings().unwrap_or_default();
    let config = crate::llm::LlmConfig::from_settings(&settings);
    let st = Stance::from_label(&stance);
    let strict = Strictness::from_label(&strictness);

    let result = analyze::review_contract(&config, &numbered, st, strict, &contract_type_hint)
        .await
        .map_err(|e| format!("合同审查失败:{}", e))?;

    let contract_name = contract_name_from_path(&docx_path);
    let opinion_md = report::build_opinion_md(&result, st.cn_short(), strict.cn_short(), &[]);

    Ok(ContractReviewResponse {
        contract_name,
        paragraph_count: parsed.non_empty_count(),
        result,
        opinion_md,
    })
}

/// 导出审查意见书 Word。前端把 `review_contract_docx` 拿到的 `result` 原样回传。
#[tauri::command]
pub async fn export_contract_opinion_docx(
    result: ContractReviewResult,
    contract_name: String,
    stance: String,
    strictness: String,
    save_path: String,
) -> Result<String, String> {
    let st = Stance::from_label(&stance);
    let strict = Strictness::from_label(&strictness);
    let bytes = report::build_opinion_docx(
        &result,
        &contract_name,
        st.cn_short(),
        strict.cn_short(),
        &[],
    )?;
    std::fs::write(&save_path, &bytes).map_err(|e| format!("写审查意见书 docx 失败:{}", e))?;
    Ok(save_path)
}

/// 导出修订批注版 docx 的结果摘要(给前端展示落痕情况)。
#[derive(Debug, Serialize)]
pub struct RedlineSummary {
    /// 落了行内修订痕迹(w:ins/w:del)的条数
    pub applied_inline: usize,
    /// 落了整段批注的条数
    pub applied_comment: usize,
    /// 未能落入正文、只在审查意见书提示的条目
    pub skipped: Vec<String>,
    /// 写盘路径
    pub saved_path: String,
}

/// 导出修订批注版 Word:在**原合同 docx** 上落 P2 整段批注 + P3 行内修订痕迹。
/// `src_docx_path` 是审查时上传的原合同;`result` 由前端从 review 结果原样回传。
#[tauri::command]
pub async fn export_contract_redline_docx(
    src_docx_path: String,
    result: ContractReviewResult,
    author: String,
    save_path: String,
) -> Result<RedlineSummary, String> {
    let author = if author.trim().is_empty() {
        "合同审查(CaseBoard)".to_string()
    } else {
        author.trim().to_string()
    };
    let outcome = redline::build_redlined_docx(&src_docx_path, &result, &author)?;
    std::fs::write(&save_path, &outcome.docx)
        .map_err(|e| format!("写修订批注版 docx 失败:{}", e))?;
    Ok(RedlineSummary {
        applied_inline: outcome.applied_inline,
        applied_comment: outcome.applied_comment,
        skipped: outcome.skipped,
        saved_path: save_path,
    })
}

/// 把旧版 `.doc` / `.rtf` / `.odt` 合同转成 `.docx`,返回转换后文件路径(临时目录)。
///
/// 合同审查引擎(parse/redline)只吃 `.docx` 结构;拖入旧 `.doc` 时前端先调本命令转换再审查。
/// macOS 用系统 `textutil`(自带);Windows/Linux 尝试 LibreOffice(`soffice`),没装则透传
/// 清晰错误引导用户在 Word 里「另存为 .docx」。失败真错透传(坑 #8)。
#[tauri::command]
pub async fn convert_doc_to_docx(src_path: String) -> Result<String, String> {
    let src = std::path::Path::new(&src_path);
    if !src.exists() {
        return Err(format!("文件不存在:{}", src_path));
    }
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("合同")
        .to_string();
    let out_dir = std::env::temp_dir().join("caseboard_contract_convert");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("创建临时目录失败:{}", e))?;
    let out_path = out_dir.join(format!("{}.docx", stem));

    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("textutil")
            .arg("-convert")
            .arg("docx")
            .arg(&src_path)
            .arg("-output")
            .arg(&out_path)
            .output()
            .map_err(|e| format!("调 textutil 失败:{}", e))?;
        if !out.status.success() {
            return Err(format!(
                "textutil 转换失败:{}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Windows/Linux:尝试 LibreOffice(soffice --headless --convert-to docx),
        // 输出 <outdir>/<stem>.docx。裸名 spawn 不经 shell,挨个候选试(坑 #21 外部命令名跨平台)。
        let candidates: &[&str] = if cfg!(windows) {
            &["soffice.exe", "soffice", "soffice.com"]
        } else {
            &["libreoffice", "soffice"]
        };
        let mut ok = false;
        let mut last_err = String::new();
        for bin in candidates {
            let mut cmd = std::process::Command::new(bin);
            cmd.arg("--headless")
                .arg("--convert-to")
                .arg("docx")
                .arg("--outdir")
                .arg(&out_dir)
                .arg(&src_path);
            // Windows 下隐藏 LibreOffice 控制台窗口,避免转换时闪黑框。
            crate::proc_util::hide_console_window_std(&mut cmd);
            match cmd.output() {
                Ok(o) if o.status.success() => {
                    ok = true;
                    break;
                }
                Ok(o) => last_err = String::from_utf8_lossy(&o.stderr).to_string(),
                Err(e) => last_err = e.to_string(),
            }
        }
        if !ok {
            return Err(format!(
                "本机未找到可用的转换工具(LibreOffice / soffice),无法把旧 .doc 自动转 .docx。\
                 请在 Word 里把合同「另存为 .docx」后再拖入。详情:{}",
                last_err
            ));
        }
    }

    if !out_path.exists() {
        return Err(format!("转换后未生成 docx:{}", out_path.display()));
    }
    out_path
        .to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "转换后路径含非法 UTF-8".to_string())
}
