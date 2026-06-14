-- 2026-06-13 · 我方代理立场(agg_our_side)
-- 反馈人「Pure小法师」:报告与 AI 输出要按我方代理地位(原告方/被告方)调整侧重点。
--
-- 取值:'原告方' / '被告方' / '第三人' / '反诉混合' / NULL(未知)。
--   - LLM 全局抽从 is_our_side=true 的当事人 role 推断,写入本列。
--   - 用户在详情页改的值走 user_overrides_json overlay(fields.agg_our_side),**不进本列**
--     (与其它 agg_* 字段同哲学:LLM 抽的留原位,用户改的叠加在渲染/喂 LLM 层)。
ALTER TABLE cases ADD COLUMN agg_our_side TEXT;
