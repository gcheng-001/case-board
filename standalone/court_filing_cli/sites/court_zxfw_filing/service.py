"""全国法院"一张网"在线立案服务 facade。"""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import TYPE_CHECKING, Any

from playwright.sync_api import Page

from .filing_steps import FilingStepsMixin
from .party_info_handler import PartyInfoHandlerMixin
from .progress_reporter import ProgressReporterMixin

if TYPE_CHECKING:
    from plugins.court_filing_http.api_service import CourtZxfwFilingApiService

logger = logging.getLogger("court_filing_cli")


class CourtZxfwFilingService(FilingStepsMixin, PartyInfoHandlerMixin, ProgressReporterMixin):  # pragma: no cover
    """
    全国法院"一张网"在线立案服务

    前置条件: 需要已登录的 Page 对象（由 CourtZxfwService.login() 完成）

    民事一审流程（6步）:
    1. 选择受理法院 → 2. 阅读须知 → 3. 选择案由 → 4. 上传材料 → 5. 完善信息 → 6. 预览

    申请执行流程（5步）:
    1. 选择受理法院 → 2. 阅读须知 → 3. 上传材料(含执行依据) → 4. 完善信息 → 5. 预览
    """

    BASE_URL = "https://zxfw.court.gov.cn/zxfw"
    CASE_TYPE_URL = f"{BASE_URL}/#/pagesWsla/pc/zxla/pick-case-type/index"

    PROVINCE_CODES: dict[str, str] = {
        "北京市": "110000",
        "天津市": "120000",
        "河北省": "130000",
        "山西省": "140000",
        "内蒙古自治区": "150000",
        "辽宁省": "210000",
        "吉林省": "220000",
        "黑龙江省": "230000",
        "上海市": "310000",
        "江苏省": "320000",
        "浙江省": "330000",
        "安徽省": "340000",
        "福建省": "350000",
        "江西省": "360000",
        "山东省": "370000",
        "河南省": "410000",
        "湖北省": "420000",
        "湖南省": "430000",
        "广东省": "440000",
        "广西壮族自治区": "450000",
        "海南省": "460000",
        "重庆市": "500000",
        "四川省": "510000",
        "贵州省": "520000",
        "云南省": "530000",
        "西藏自治区": "540000",
        "陕西省": "610000",
        "甘肃省": "620000",
        "青海省": "630000",
        "宁夏回族自治区": "640000",
        "新疆维吾尔自治区": "650000",
    }

    EXEC_SECTION_MAP: dict[str, str] = {
        "plaintiffs": "申请执行人信息",
        "defendants": "被执行人信息",
    }

    CIVIL_SECTION_MAP: dict[str, str] = {
        "plaintiffs": "原告信息",
        "defendants": "被告信息",
        "third_parties": "第三人信息",
    }

    CIVIL_UPLOAD_SLOT_KEYWORDS: list[tuple[str, tuple[str, ...]]] = [
        ("0", ("起诉状", "诉状")),
        ("1", ("当事人身份证明", "身份证明", "营业执照", "身份证")),
        ("2", ("委托代理人委托手续和身份材料", "授权委托书", "律师执业证", "委托代理")),
        ("3", ("证据目录及证据材料", "证据目录", "证据材料")),
        ("4", ("送达地址确认书", "送达地址")),
        ("5", ("其他材料",)),
    ]

    EXEC_UPLOAD_SLOT_KEYWORDS: list[tuple[str, tuple[str, ...]]] = [
        ("0", ("执行申请书", "申请执行书", "申请书")),
        ("1", ("执行依据文书", "执行依据", "判决书", "裁定书", "调解书")),
        ("2", ("授权委托书及代理人身份证明", "授权委托书", "律师执业证", "代理人身份证明")),
        ("3", ("申请人身份材料", "身份证明", "营业执照", "身份证")),
        ("4", ("送达地址确认书", "送达地址")),
    ]

    def __init__(
        self,
        page: Page,
        *,
        save_debug: bool = False,
        debug_dir: str | None = None,
    ) -> None:
        self.page = page
        self.save_debug = save_debug
        self.debug_dir = debug_dir

    @classmethod
    def resolve_province_code(cls, province: str) -> str:
        """将省份名称解析为行政区划代码。

        支持精确匹配和模糊匹配（如 "广西" → "广西壮族自治区"）。
        找不到时抛出 ValueError，避免静默回退到错误省份。
        """
        province = (province or "").strip()
        if not province:
            supported = "、".join(sorted(cls.PROVINCE_CODES.keys()))
            raise ValueError(f"省份为空，无法进入正确法院列表。支持的省份：{supported}")

        if province in cls.PROVINCE_CODES:
            return cls.PROVINCE_CODES[province]

        for full_name, code in cls.PROVINCE_CODES.items():
            if province in full_name or full_name.startswith(province):
                return code

        supported = "、".join(sorted(cls.PROVINCE_CODES.keys()))
        raise ValueError(f"不支持的省份「{province}」，未找到对应行政区划代码。支持的省份：{supported}")

    # ==================== 主入口 ====================

    def file_case(self, case_data: dict[str, Any], token: str | None = None) -> dict[str, Any]:  # pragma: no cover
        """执行民事一审在线立案全流程。"""
        filing_engine = self._resolve_filing_engine(case_data)
        api_error: Exception | None = None
        if filing_engine == "api":
            self._report_progress(
                case_data,
                phase="http",
                stage="http.start",
                message="HTTP主链路：开始民事一审立案",
            )
            if not token:
                api_error = ValueError("HTTP立案缺少登录令牌")
                if not self._allow_playwright_fallback(case_data):
                    raise ValueError("接口立案失败: %(error)s" % {"error": api_error}) from api_error
                self._report_progress(
                    case_data,
                    phase="http",
                    stage="http.failed",
                    level="error",
                    message=f"HTTP主链路失败: {api_error}",
                )
                logger.warning("HTTP立案缺少登录令牌，回退 Playwright")
            else:
                try:
                    from plugins import has_court_filing_api_plugin

                    if not has_court_filing_api_plugin():
                        raise ImportError("HTTP链路插件未安装")

                    self._report_progress(
                        case_data,
                        phase="http",
                        stage="http.submit",
                        message="HTTP主链路：正在提交一张网草稿",
                    )
                    from plugins.court_filing_http.api_service import CourtZxfwFilingApiService

                    api_svc = CourtZxfwFilingApiService(token)
                    result: dict[str, object] = api_svc.file_civil_case_sync(case_data)
                    self._report_progress(
                        case_data,
                        phase="http",
                        stage="http.success",
                        message="HTTP主链路提交成功",
                    )
                    logger.info("HTTP立案成功: %s", result)
                    return result
                except Exception as api_err:
                    api_error = api_err
                    self._report_progress(
                        case_data,
                        phase="http",
                        stage="http.failed",
                        level="error",
                        message=f"HTTP主链路失败: {api_err}",
                    )
                    if not self._allow_playwright_fallback(case_data):
                        logger.error("HTTP立案失败: %s", api_err, exc_info=True)
                        raise ValueError("接口立案失败: %(error)s" % {"error": api_err}) from api_err
                    logger.warning("HTTP立案失败，回退 Playwright: %s", api_err, exc_info=True)

        self._report_progress(
            case_data,
            phase="playwright",
            stage="playwright.start",
            message="进入Playwright回退流程（民事一审）",
        )

        court_name: str = case_data["court_name"]
        cause_of_action: str = case_data["cause_of_action"]

        logger.info("=" * 60)
        logger.info("开始民事一审立案: 法院=%s, 案由=%s", court_name, cause_of_action)
        logger.info("=" * 60)

        try:
            province = case_data.get("province", "")
            province_code = self.resolve_province_code(province)

            self._report_progress(
                case_data,
                phase="playwright",
                stage="playwright.step.open_case_type",
                message="回退阶段：打开案件类型页",
            )
            self._open_case_type_page("民事一审", province_code)
            self._report_progress(
                case_data, phase="playwright", stage="playwright.step.select_court", message="回退阶段：选择受理法院"
            )
            self._step1_select_court(
                court_name,
                city_name=case_data.get("city", ""),
                district_name=case_data.get("district", ""),
            )
            self._report_progress(
                case_data, phase="playwright", stage="playwright.step.read_notice", message="回退阶段：确认立案须知"
            )
            self._step2_read_notice()
            self._report_progress(
                case_data, phase="playwright", stage="playwright.step.select_cause", message="回退阶段：选择案由"
            )
            self._step3_select_cause(cause_of_action)
            self._report_progress(
                case_data,
                phase="playwright",
                stage="playwright.step.upload_materials",
                message="回退阶段：上传诉讼材料",
            )
            self._step4_upload_materials(case_data.get("materials", {}), is_execution=False)
            self._report_progress(
                case_data,
                phase="playwright",
                stage="playwright.step.fill_case_info",
                message="回退阶段：完善当事人和代理人信息",
            )
            self._step5_complete_info(case_data, section_map=self.CIVIL_SECTION_MAP)
            self._report_progress(
                case_data, phase="playwright", stage="playwright.step.next", message="回退阶段：进入预览页"
            )
            self._click_next_step()
            self._step6_preview_submit()
            self._report_progress(
                case_data,
                phase="playwright",
                stage="playwright.success",
                message="Playwright回退流程完成（已到预览页）",
            )

            logger.info("民事一审立案流程执行完成")
            return {"success": True, "message": "立案流程执行完成（已到预览页，未提交）", "url": self.page.url}

        except Exception as e:
            self._report_progress(
                case_data,
                phase="playwright",
                stage="playwright.failed",
                level="error",
                message=f"Playwright回退失败: {e}",
            )
            logger.error("民事一审立案失败: %s", e, exc_info=True)
            if self.save_debug:
                self._save_screenshot("error_civil_filing")
            merged_error = str(e)
            if api_error is not None:
                merged_error = f"HTTP主链路失败({api_error})，且Playwright回退失败({e})"
            raise ValueError("立案失败: %(error)s" % {"error": merged_error}) from e

    def convert_element_document(self, case_data: dict[str, Any], source_path: str, output_dir: str) -> dict[str, Any]:
        """使用人民法院在线服务的要素式/智能识别能力转换传统诉状。

        只到法院端生成并下载结果，不提交立案。
        """
        court_name: str = case_data["court_name"]
        cause_of_action: str = case_data.get("cause_of_action", "")
        source = Path(source_path)
        if not source.exists():
            raise ValueError(f"源诉状不存在: {source_path}")

        output = Path(output_dir)
        output.mkdir(parents=True, exist_ok=True)

        self._report_progress(
            case_data,
            phase="playwright",
            stage="element.open_case_type",
            message="打开人民法院在线服务民事一审入口",
        )
        province_code = self.resolve_province_code(case_data.get("province", ""))
        self._open_case_type_page("民事一审", province_code)
        self._report_progress(
            case_data,
            phase="playwright",
            stage="element.select_court",
            message="选择受理法院",
        )
        self._step1_select_court(
            court_name,
            city_name=case_data.get("city", ""),
            district_name=case_data.get("district", ""),
        )
        self._report_progress(
            case_data,
            phase="playwright",
            stage="element.enter_element_mode",
            message="进入法院端要素式/智能识别入口",
        )
        self._step2_read_notice(has_prepared_doc=False, element_strategy="accept")
        self._handle_popups(element_strategy="accept")
        if self.save_debug:
            self._save_screenshot("element_after_notice")

        if cause_of_action:
            self._report_progress(
                case_data,
                phase="playwright",
                stage="element.select_cause",
                message="选择案由",
            )
            self._select_element_cause(cause_of_action)

        self._report_progress(
            case_data,
            phase="playwright",
            stage="element.upload_source",
            message="上传传统诉状并等待法院端识别",
        )
        self._upload_source_for_element_convert(source)
        if self.save_debug:
            self._save_screenshot("element_after_upload")

        self._report_progress(
            case_data,
            phase="playwright",
            stage="element.generate",
            message="触发法院端生成要素式文书",
        )
        self._trigger_element_generation()
        self._wait_for_element_conversion()
        self._handle_popups(element_strategy="accept")
        if self.save_debug:
            self._save_screenshot("element_after_generate")

        self._report_progress(
            case_data,
            phase="playwright",
            stage="element.download",
            message="下载法院端生成结果",
        )
        try:
            download_path = self._download_generated_element_document(output)
        except ValueError:
            snapshot_path = self._save_official_element_snapshot(output)
            return {
                "success": True,
                "message": "法院端要素式识别和表单回填已完成，等待生成可审阅草稿",
                "url": self.page.url,
                "download_path": "",
                "official_snapshot_path": str(snapshot_path),
                "draft_required": True,
            }
        return {
            "success": True,
            "message": "法院端要素式文书已生成并下载",
            "url": self.page.url,
            "download_path": str(download_path),
        }

    def _trigger_element_generation(self) -> None:
        label = "传统文本转要素式文本"
        text = self.page.get_by_text(label, exact=True)
        if not text.count():
            raise ValueError(f"法院端页面未找到按钮「{label}」")
        button = text.first.locator("xpath=ancestor-or-self::uni-button[1]")
        if not button.count():
            button = text.first

        for _ in range(5):
            disabled = button.get_attribute("disabled")
            class_name = button.get_attribute("class") or ""
            if disabled not in {"", "true", "disabled"} and "disabled" not in class_name.lower():
                break
            if disabled is None and "disabled" not in class_name.lower():
                break
            self.page.wait_for_timeout(1000)
        if button.get_attribute("disabled") in {"", "true", "disabled"}:
            invoked = button.evaluate(
                """element => {
                    let vm = element.__vue__;
                    while (vm && !(vm.$options && vm.$options.methods && vm.$options.methods.nextStep)) vm = vm.$parent;
                    if (!vm) return false;
                    vm.nextStep('ysht');
                    return true;
                }"""
            )
            if not invoked:
                raise ValueError("法院端转换按钮被禁用，且未找到官方转换动作")
            logger.info("已调用法院端传统文本转要素式文本动作")
        else:
            button.click(timeout=10000)
        self._random_wait(2, 3)

    def _save_official_element_snapshot(self, output_dir: Path) -> Path:
        snapshot = self.page.evaluate(
            """() => ({
                url: location.href,
                title: document.title,
                fields: Array.from(document.querySelectorAll('input, textarea'))
                    .map((element) => ({
                        value: element.value || '',
                        placeholder: element.getAttribute('placeholder') || '',
                        label: (element.closest('.uni-forms-item')?.querySelector('.uni-forms-item__label')?.textContent || '').trim(),
                    }))
                    .filter((item) => item.value || item.label),
                pageText: (document.body.innerText || '').slice(0, 50000),
            })"""
        )
        target = output_dir / "法院端要素式回填快照.json"
        target.write_text(json.dumps(snapshot, ensure_ascii=False, indent=2), encoding="utf-8")
        logger.info("法院端要素式回填快照已保存: %s", target)
        return target

    def _wait_for_element_conversion(self) -> None:
        overlay = self.page.get_by_text("示范文本回填中", exact=False)
        try:
            overlay.first.wait_for(state="visible", timeout=15000)
            overlay.first.wait_for(state="hidden", timeout=180000)
        except Exception as exc:
            raise ValueError(f"法院端要素式回填未在限定时间内完成: {exc}") from exc
        self._wait_until_idle()

    def _select_element_cause(self, cause_of_action: str) -> None:
        """Select a cause card on the court's element-style filing page."""
        matches = self.page.get_by_text(cause_of_action, exact=True)
        selected = False
        for index in range(matches.count()):
            item = matches.nth(index)
            if item.is_visible():
                item.click(timeout=10000)
                selected = True
                break
        if not selected:
            raise ValueError(f"法院要素式页面未找到案由「{cause_of_action}」")
        self._random_wait(0.5, 1)
        self._click_first_visible_button(("下一步",), required=True)
        self._wait_until_idle()
        if self.save_debug:
            self._save_screenshot("element_after_cause")

    def _upload_source_for_element_convert(self, source: Path) -> None:
        upload_buttons = self.page.locator(".fd-btn-add, uni-button:has-text('上传'), uni-button:has-text('选择文件')")
        if not upload_buttons.count():
            raise ValueError("法院端页面未找到传统诉状上传入口")
        before = self.page.locator(".fd-file-name").count()
        with self.page.expect_file_chooser() as fc_info:
            upload_buttons.first.click(timeout=10000)
        fc_info.value.set_files(str(source))
        self._wait_until_idle()
        after = self.page.locator(".fd-file-name").count()
        if after <= before:
            logger.warning("上传后未检测到文件列表增加，继续尝试法院端识别")

    def _download_generated_element_document(self, output_dir: Path) -> Path:
        buttons = (
            "下载",
            "下载文书",
            "导出",
            "导出Word",
            "导出 word",
            "保存",
            "预览下载",
        )
        for label in buttons:
            locator = self.page.locator(f"uni-button:has-text('{label}'), button:has-text('{label}'), a:has-text('{label}')")
            for index in range(locator.count()):
                btn = locator.nth(index)
                try:
                    if not btn.is_visible():
                        continue
                    with self.page.expect_download(timeout=30000) as download_info:
                        btn.click(timeout=10000)
                    download = download_info.value
                    suggested = download.suggested_filename or "法院端要素式文书.docx"
                    target = self._unique_download_path(output_dir, suggested)
                    download.save_as(str(target))
                    return target
                except Exception as exc:
                    logger.debug("点击下载按钮失败 label=%s index=%d error=%s", label, index, exc)
                    continue
        raise ValueError("法院端已进入生成流程，但未找到可用的下载按钮")

    def _click_first_visible_button(self, labels: tuple[str, ...], *, required: bool) -> bool:
        for label in labels:
            locator = self.page.locator(f"uni-button:has-text('{label}'), button:has-text('{label}'), a:has-text('{label}')")
            for index in range(locator.count()):
                try:
                    item = locator.nth(index)
                    if item.is_visible():
                        item.click(timeout=10000)
                        self._random_wait(2, 3)
                        return True
                except Exception:
                    continue
        if required:
            raise ValueError(f"未找到按钮: {', '.join(labels)}")
        return False

    def _wait_until_idle(self) -> None:
        try:
            self.page.locator("text=加载中").wait_for(state="hidden", timeout=90000)
        except Exception:
            pass
        self._random_wait(3, 5)

    @staticmethod
    def _unique_download_path(output_dir: Path, filename: str) -> Path:
        safe = "".join("_" if c in '/\\:*?"<>|\n\r\t' else c for c in filename).strip()
        if not safe:
            safe = "法院端要素式文书.docx"
        target = output_dir / safe
        if not target.exists():
            return target
        stem = target.stem
        suffix = target.suffix
        for i in range(1, 100):
            candidate = output_dir / f"{stem}_{i}{suffix}"
            if not candidate.exists():
                return candidate
        return output_dir / f"{stem}_{int(__import__('time').time())}{suffix}"

    def file_execution(self, case_data: dict[str, Any], token: str | None = None) -> dict[str, Any]:  # pragma: no cover
        """执行申请执行在线立案全流程。"""
        filing_engine = self._resolve_filing_engine(case_data)
        api_error: Exception | None = None
        if filing_engine == "api":
            self._report_progress(case_data, phase="http", stage="http.start", message="HTTP主链路：开始申请执行立案")
            if not token:
                api_error = ValueError("HTTP立案缺少登录令牌")
                if not self._allow_playwright_fallback(case_data):
                    raise ValueError("接口立案失败: %(error)s" % {"error": api_error}) from api_error
                self._report_progress(
                    case_data, phase="http", stage="http.failed", level="error", message=f"HTTP主链路失败: {api_error}"
                )
                logger.warning("HTTP立案缺少登录令牌，回退 Playwright")
            else:
                try:
                    from plugins import has_court_filing_api_plugin

                    if not has_court_filing_api_plugin():
                        raise ImportError("HTTP链路插件未安装")

                    self._report_progress(
                        case_data, phase="http", stage="http.submit", message="HTTP主链路：正在提交一张网草稿"
                    )
                    from plugins.court_filing_http.api_service import CourtZxfwFilingApiService

                    api_svc = CourtZxfwFilingApiService(token)
                    result: dict[str, object] = api_svc.file_execution_sync(case_data)
                    self._report_progress(case_data, phase="http", stage="http.success", message="HTTP主链路提交成功")
                    logger.info("HTTP立案成功: %s", result)
                    return result
                except Exception as api_err:
                    api_error = api_err
                    self._report_progress(
                        case_data,
                        phase="http",
                        stage="http.failed",
                        level="error",
                        message=f"HTTP主链路失败: {api_err}",
                    )
                    if not self._allow_playwright_fallback(case_data):
                        logger.error("HTTP立案失败: %s", api_err, exc_info=True)
                        raise ValueError("接口立案失败: %(error)s" % {"error": api_err}) from api_err
                    logger.warning("HTTP立案失败，回退 Playwright: %s", api_err, exc_info=True)

        self._report_progress(
            case_data, phase="playwright", stage="playwright.start", message="进入Playwright回退流程（申请执行）"
        )

        court_name: str = case_data["court_name"]

        logger.info("=" * 60)
        logger.info("开始申请执行立案: 法院=%s", court_name)
        logger.info("=" * 60)

        try:
            province = case_data.get("province", "")
            province_code = self.resolve_province_code(province)

            self._report_progress(
                case_data,
                phase="playwright",
                stage="playwright.step.open_case_type",
                message="回退阶段：打开案件类型页",
            )
            self._open_case_type_page("申请执行", province_code)
            self._report_progress(
                case_data, phase="playwright", stage="playwright.step.select_court", message="回退阶段：选择受理法院"
            )
            self._step1_select_court(
                court_name,
                city_name=case_data.get("city", ""),
                district_name=case_data.get("district", ""),
            )
            self._report_progress(
                case_data, phase="playwright", stage="playwright.step.read_notice", message="回退阶段：确认立案须知"
            )
            self._step2_read_notice(has_prepared_doc=False)
            self._report_progress(
                case_data,
                phase="playwright",
                stage="playwright.step.select_execution_basis",
                message="回退阶段：填写执行依据信息",
            )
            self._step_exec_select_basis(case_data)
            self._report_progress(
                case_data,
                phase="playwright",
                stage="playwright.step.upload_materials",
                message="回退阶段：上传执行材料",
            )
            self._step4_upload_materials(case_data.get("materials", {}), is_execution=True)
            self._report_progress(
                case_data,
                phase="playwright",
                stage="playwright.step.fill_case_info",
                message="回退阶段：完善当事人和代理人信息",
            )
            self._step5_complete_info(case_data, section_map=self.EXEC_SECTION_MAP)
            self._report_progress(
                case_data,
                phase="playwright",
                stage="playwright.step.fill_execution_target",
                message="回退阶段：填写执行标的信息",
            )
            self._fill_execution_target_info(case_data)
            self._report_progress(
                case_data, phase="playwright", stage="playwright.step.next", message="回退阶段：进入预览页"
            )
            self._click_next_step()
            self._step6_preview_submit()
            self._report_progress(
                case_data,
                phase="playwright",
                stage="playwright.success",
                message="Playwright回退流程完成（已到预览页）",
            )

            logger.info("申请执行立案流程执行完成")
            return {
                "success": True,
                "message": "申请执行流程执行完成（已到预览页，未提交）",
                "url": self.page.url,
            }

        except Exception as e:
            self._report_progress(
                case_data,
                phase="playwright",
                stage="playwright.failed",
                level="error",
                message=f"Playwright回退失败: {e}",
            )
            logger.error("申请执行立案失败: %s", e, exc_info=True)
            if self.save_debug:
                self._save_screenshot("error_exec_filing")
            merged_error = str(e)
            if api_error is not None:
                merged_error = f"HTTP主链路失败({api_error})，且Playwright回退失败({e})"
            raise ValueError("申请执行立案失败: %(error)s" % {"error": merged_error}) from e
