//! 案件 AI 助手的 system / task prompt 模板。
//!
//! 核心约束(CLAUDE.md 第 4 节):
//!   - 不得虚构事实、证据、法规、判例或法律依据
//!   - 不得把未经确认的事实写成确定事实
//!   - 涉及最终法律判断时由律师决定
//!
//! 这些约束**必须**写进 system prompt,而不是依赖 LLM 默认行为。
//! DeepSeek 在写法律内容时会有幻觉法条编号、虚构判例名的倾向。

use super::context::TaskType;

/// 把 task_type 翻译成给 LLM 的用户层指令(拼到 user message 前面)。
///
/// 自由问 (FreeChat) 时返回 None,直接用用户原话。
pub fn task_user_prompt(task: TaskType) -> Option<&'static str> {
    match task {
        TaskType::FreeChat => None,
        // 工具/分析型任务(走 agent_loop 工具链路)。V0.3.3 起 6 个生成型 chip 已删,
        // 「写文书」类需求统一由用户在聊天里发指令 → FreeChat → save_artifact 落档。
        TaskType::CompileLegalBasis => Some(
            "请整理一份「法律依据清单」MD 文档。\n\
             \n\
             【第一步 · 先判断案件阶段】看案件快照里的「工作流状态」「判决/调解/执行结果」「案件总状态」:\n\
             - 若已有生效判决书/调解书,或处于【执行中 / 已结案 / 执行阶段】\n\
               → 实体争议已由法院确认,**不要再分析实体争议焦点**;只整理**执行阶段**所需法律依据:\n\
               申请执行依据、财产调查与查控、追加/变更被执行人、参与分配、执行异议与复议、\n\
               终本与恢复执行、拒执罪等。\n\
             - 若仍在【立案 / 审理 / 一审 / 上诉期 / 二审】等实体审理阶段\n\
               → 按本案**争议焦点**整理实体 + 程序法律依据。\n\
             - 阶段不明时默认按审理阶段,并注明\"阶段判断:依据不足,暂按审理阶段\"。\n\
             在文档开头用一行写明你判断的阶段。\n\
             \n\
             【第二步 · 查证(凡引用必先工具验证,不得编条号)】\n\
             1. 先用 `search_local_kb` 查作者本地知识库已有整理(优先复用,省积分)\n\
             2. 用 `search_laws` 定位到确切法条(拿到 fgmc / 条号 / fgid),再用 `get_law_article(fgid+ftnum)` 取**那一条**全文进上下文核对 —— **整部法规不要喂进上下文**(费 token)\n\
             3. 关键词不准时用 `law_vector_search` 语义检索兜底;拿到 fgid + 条号后同样走 `get_law_article` 取单条\n\
             4. 默认不要拉整部法规;确需看法规整体结构时才用 `get_regulation_detail`(默认只回元信息+预览,要某条仍走 get_law_article)\n\
             \n\
             【输出结构】\n\
             - 执行阶段:按「执行事项」分组(申请执行依据 / 财产查控 / 追加被执行人 / 拒执 ...),\n\
               每组列:法律依据(法规名 第 X 条)+ 原文摘录 + 适用要点。\n\
             - 审理阶段:按「争议焦点」分组:\n\
               - 一、争议焦点 1:<焦点描述>\n\
                 - 法律依据(法规名 第 X 条):<原文摘录,标出条号>\n\
                 - 适用要件:<构成要件分析>\n\
                 - 司法解释/相关规定(若有)\n\
               - 二、争议焦点 2:...\n\
             - 末尾:需注意的程序性规定\n\
             - <CITATIONS> 块列出所有引用条款的精确出处\n\
             **不得编条号** — 凡引用必先 `search_laws` 验证;调用工具失败仍写不出来时,明确说\"现有数据源未命中该条款\"。",
        ),
        TaskType::SimulateOpposition => Some(
            "请做一次「模拟对抗」:站在**对方当事人**立场,推演对方最可能的抗辩/进攻,再给我方应对。\n\
             \n\
             【第一步 · 定位双方与阶段】从案件快照判断我方代理哪一方、对方是谁,再判断阶段:\n\
             - 审理阶段:对方=诉讼相对方,对抗 = 实体抗辩 + 程序异议(管辖/主体/证据/时效等)\n\
             - 执行阶段:对方=被执行人/案外人,对抗 = 执行异议 / 案外人异议 / 不予执行 / 管辖异议等\n\
             \n\
             【第二步 · 查证支持对方的依据】(凡引用必先工具验证)\n\
             1. `search_local_kb` 查本地是否有相关整理\n\
             2. `search_laws` / `get_law_article` 拿对方可能援引的法条\n\
             3. `search_cases_authority` / `search_cases_normal` 找支持对方观点的类案\n\
             4. 关键词不准时用 `law_vector_search` / `case_vector_search` 兜底\n\
             \n\
             【输出结构】逐个争议焦点(执行阶段则逐个对抗事由):\n\
             - 争议焦点 X:<焦点描述>\n\
               - 🔴 对方可能主张:<站对方立场的最强论点,像对方律师那样狠>\n\
               - 对方依据:<法条 第 X 条 / 类案案号,均须工具验证过>\n\
               - 🟢 我方应对:<针对性反驳 + 我方依据 + 如何削弱对方>\n\
             - 末尾:整体风险提示(对方最可能突破的薄弱点 + 我方需补强处)\n\
             - <CITATIONS> 块列出所有引用法条/案号的精确出处\n\
             **不得编条号/案号** — 凡引用必先工具验证。这是攻防推演、非最终法律意见,关键判断请律师把关。",
        ),
        TaskType::FindSimilarCases => Some(
            "请基于本案事实和争议焦点,做一次「类案检索 + 对本案的支持度分析」。\n\
             目标不是中立罗列判决,而是:**找到相似案例,判断它们对「我方诉讼请求」是支持还是不利,并指出风险点。**\n\
             \n\
             【第一步 · 定位本案】从案件快照读出:我方代理哪一方、核心诉讼请求、案由、争议焦点、地域(法院所在地)。\n\
             \n\
             【第二步 · 检索(凡引用必先工具验证)】\n\
             1. 先用 `search_local_kb` 查作者本地是否已整理过类案(优先复用,省积分)\n\
             2. `search_cases_authority` 拿权威/指导/公报案例(引用价值最高)\n\
             3. `search_cases_normal` 拿普通案例补充\n\
             4. 关键词不准时用 `case_vector_search` 语义检索兜底\n\
             5. 命中后挑 **5-6 个**最相关的用 `get_case_detail` 拿全文核对裁判要旨\n\
             \n\
             【地域优先(无服务端地域过滤,自己筛)】\n\
             - 检索工具**当前只接关键词,没有地域参数**;不要假设能按省市过滤。\n\
             - 做法:关键词里带上本案地域(如「温州」「浙江」)提高命中相关度;拿到结果后**从每条的 `court` 字段自己判断**,优先选本案同省/同市(温州/浙江)的判决,再补外地高相关案例。\n\
             \n\
             【输出结构】\n\
             - 一、检索概述:用了哪些关键词、命中多少、最终精选几条、地域分布\n\
             - 二、类案逐条(5-6 个,本地/同省优先):\n\
               - 类案 N:<案号> · <法院>(<权威等级,若来自权威库>)\n\
                 - 案情简述:<2-3 句>\n\
                 - 裁判要旨:<法院核心说理>\n\
                 - 与本案:相似点 / 关键区别点\n\
                 - 对我方:🟢 支持 / 🔴 不利 / ⚪ 中性 —— 一句话说清这条对我方诉求意味着什么\n\
             - 三、对我方诉讼请求的支持可能性:<综合类案裁判口径,给出偏乐观/中性/偏谨慎的判断 + 理由>\n\
             - 四、风险点 / 需补强:<对方可能援引的不利类案、我方事实与胜诉案例的差距、需要补的证据或论证>\n\
             - <CITATIONS> 块列出所有引用案例的案号 + 法院\n\
             **不得编案号** — 凡引用必先工具验证;case_no 必须复制元典返回的原值。这是检索辅助分析、非最终法律意见,最终判断请律师把关。",
        ),
        TaskType::VerifyMyDraft => Some(
            "用户在 attached 文档里给出了一份起诉状/合同/分析草稿,需要你核校里面引用的法规和案例。\n\
             流程:\n\
             1. 用 `list_case_docs` + `read_case_doc` 读 attached 文档全文\n\
             2. 把可能含引用的段落整段塞进 `verify_legal_citations` 检测\n\
             3. 拿到 citations 数组,逐条处理:\n\
                - verdict=\"一致\" → 标 ✅ 放心引用\n\
                - verdict=\"不一致\" → 标 ⚠️,用 authoritative_content 字段给出正确写法\n\
                - verdict=\"未命中\" → 标 ❌,可能是用户凭印象编的,需要追加 `search_laws` 或 `search_cases_normal` 再确认\n\
             输出结构:\n\
             - 校验结论:<X 处引用,Y 处正确,Z 处需修正>\n\
             - 逐条问题清单:\n\
               1. 引用片段「<原文>」\n\
                  - 状态:✅/⚠️/❌\n\
                  - 元典认定:<元典查到的正确版本,若有>\n\
                  - 建议改写:<针对性建议>\n\
               2. ...\n\
             - 总体建议:<整体引用质量评估>\n\
             - <CITATIONS> 块只列已校验确认存在的引用",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_tool_task_has_concrete_template() {
        // V0.3.3:6 个生成型 chip 已删,只剩 4 个工具/分析型任务有模板。
        for task in [
            TaskType::CompileLegalBasis,
            TaskType::FindSimilarCases,
            TaskType::VerifyMyDraft,
            TaskType::SimulateOpposition,
        ] {
            let p = task_user_prompt(task);
            assert!(p.is_some(), "task {:?} 应该有模板", task);
            let p = p.unwrap();
            // 模板长度合理(不是空串;阶段感知 / 模拟对抗模板更长,放宽到 4000 字节)
            assert!(p.len() > 50, "task {:?} 模板太短", task);
            assert!(p.len() < 4000, "task {:?} 模板太长", task);
        }
    }

    #[test]
    fn v0_2_tool_tasks_mention_tools() {
        // V0.2 工具任务模板里应该出现具体工具名
        let p = task_user_prompt(TaskType::CompileLegalBasis).unwrap();
        assert!(p.contains("search_laws") || p.contains("search_local_kb"));
        let p = task_user_prompt(TaskType::FindSimilarCases).unwrap();
        assert!(p.contains("search_cases_authority") || p.contains("case_vector_search"));
        let p = task_user_prompt(TaskType::VerifyMyDraft).unwrap();
        assert!(p.contains("verify_legal_citations"));
        let p = task_user_prompt(TaskType::SimulateOpposition).unwrap();
        assert!(p.contains("search_cases_authority") || p.contains("search_laws"));
    }

    #[test]
    fn free_chat_returns_none() {
        assert!(task_user_prompt(TaskType::FreeChat).is_none());
    }
}
