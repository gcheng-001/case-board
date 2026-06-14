-- 2026-06-13 · 文档级 OCR 后端覆盖(ocr_backend_override)
-- 反馈人「胡彬律师」+ 尽调实测:带大幅水印的工商调档件,VL-1.6(带排版)会把水印当正文排进去导致不可用;
-- 改用 PP-OCRv6(纯文字行)+ 去水印过滤,关键登记字段可读。
--
-- 取值:'ppocrv6' = 用户对该文档点了「去水印重新识别」→ 强制走 PP-OCRv6 + 去水印(不回退);
--       NULL = 走常规 OCR 策略(VL/MinerU 主备)。粘性:水印档案重扫仍走去水印,直到普通「重新识别」清除。
ALTER TABLE documents ADD COLUMN ocr_backend_override TEXT;
