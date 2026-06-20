"""Nedev（能迪）OA 系统适配器。

Nedev 是国内律所常用的 OA 平台,特征:
  - Vue.js SPA 前端
  - 登录页: /Nedev/ 或根路径
  - AgentAPI: GET DataServices/AgentAPI/Login?apikey=...
  - 后续接口通过 nedev_access_token 请求头鉴权

本适配器实现:
  1. AgentAPI Key 登录(支持旧账号密码/Playwright 兜底)
  2. 案件列表拉取
  3. 客户列表拉取
  4. AgentAPI 案件登记
"""
from __future__ import annotations

import json
import time
from datetime import date
from typing import Any

import httpx

from .base import OAResult, OAScriptBase


class NedevScript(OAScriptBase):
    """Nedev OA 适配器。"""

    def __init__(self, site_url: str, account: str, password: str, **kwargs: Any) -> None:
        super().__init__(site_url, account, password, **kwargs)
        self._token: str | None = None
        self._http: httpx.Client | None = None
        self._agent_api_ready = False
        self._agent_api_rejected = False

    # ─────────── 登录 ───────────

    def login(self) -> bool:
        """优先用 AgentAPI Key 登录,失败再走旧登录链路。"""
        if self._try_agent_api_login():
            return True
        if self._agent_api_rejected:
            return False
        if self._try_api_login():
            return True
        return self._try_playwright_login()

    def _api_url(self, action: str) -> str:
        return f"{self.site_url}/DataServices/AgentAPI/{action}"

    def _try_agent_api_login(self) -> bool:
        """用律师后台提供的 Agent API Key 换 token。

        兼容当前 UI: password 字段作为 API Key；account 字段也可直接填 Key。
        """
        api_key = (self.password or self.account or "").strip()
        if not api_key:
            return False

        self._http = httpx.Client(timeout=30, follow_redirects=True)
        try:
            resp = self._http.get(self._api_url("Login"), params={"apikey": api_key})
            data = resp.json()
        except Exception:
            return False

        if data.get("code") != 200:
            msg = data.get("msg") or f"HTTP {resp.status_code}"
            self._agent_api_rejected = True
            self._login_error = f"AgentAPI 登录失败: {msg}"
            self.report("agent_api_login_failed", 0, self._login_error)
            return False

        token = (data.get("data") or {}).get("token")
        if not token:
            self._login_error = "AgentAPI 登录失败: 未返回 token"
            self.report("agent_api_login_failed", 0, self._login_error)
            return False

        self._token = token
        self._agent_api_ready = True
        self._http.headers["nedev_access_token"] = token
        self.report("agent_api_login_ok", 15, "AgentAPI 登录成功")
        return True

    def _agent_get(self, action: str, **params: Any) -> Any:
        if not self._http or not self._agent_api_ready:
            raise RuntimeError("AgentAPI 未登录")
        start = time.monotonic()
        resp = self._http.get(self._api_url(action), params={k: v for k, v in params.items() if v is not None})
        elapsed = time.monotonic() - start
        if elapsed >= 2:
            self.report("agent_api_slow", 60, f"{action} 接口耗时 {elapsed:.1f}s")
        return self._unwrap_agent_response(resp)

    def _agent_post(self, action: str, payload: dict[str, Any]) -> Any:
        if not self._http or not self._agent_api_ready:
            raise RuntimeError("AgentAPI 未登录")
        start = time.monotonic()
        resp = self._http.post(self._api_url(action), json=payload)
        elapsed = time.monotonic() - start
        if elapsed >= 2:
            self.report("agent_api_slow", 80, f"{action} 接口耗时 {elapsed:.1f}s")
        return self._unwrap_agent_response(resp)

    def _unwrap_agent_response(self, resp: httpx.Response) -> Any:
        try:
            data = resp.json()
        except Exception as exc:
            raise RuntimeError(f"AgentAPI 返回非 JSON: HTTP {resp.status_code}") from exc

        code = data.get("code")
        if code == 200:
            return data.get("data")
        if code == 204:
            return None
        msg = data.get("msg") or f"AgentAPI 调用失败: code={code}"
        raise RuntimeError(str(msg))

    def _try_api_login(self) -> bool:
        """尝试通过 API 登录(Nedev 5.0 平台)。"""
        # Nedev 5.0 登录接口: POST Account/Login
        # 返回 JSON,设置 .Nedev5.0.Session cookie
        self._http = httpx.Client(timeout=30, follow_redirects=True)
        login_paths = [
            "/Account/Login",
            "/api/auth/login",
            "/api/login",
        ]
        for path in login_paths:
            try:
                resp = self._http.post(
                    f"{self.site_url}{path}",
                    data={"Account": self.account, "Password": self.password},
                    headers={"Content-Type": "application/x-www-form-urlencoded"},
                )
                if resp.status_code == 200:
                    data = resp.json()
                    # Nedev 5.0 返回 { Success: true, Data: {...} }
                    if data.get("Success") or data.get("success"):
                        self._token = data.get("Data", {}).get("Token") or data.get("token")
                        if self._token:
                            self._http.headers["Authorization"] = f"Bearer {self._token}"
                        self.report("api_login_ok", 15, "API 登录成功")
                        return True
                    # 兼容其他格式
                    token = data.get("access_token") or data.get("token")
                    if token:
                        self._token = token
                        self._http.headers["Authorization"] = f"Bearer {token}"
                        self.report("api_login_ok", 15, "API 登录成功")
                        return True
            except Exception:
                continue
        return False

    def _try_playwright_login(self) -> bool:
        """Playwright 浏览器自动化登录。"""
        self._ensure_browser(headless=True)
        page = self._page
        assert page is not None

        self.report("pw_login", 5, "正在通过浏览器登录...")

        # 打开登录页
        page.goto(self.site_url, wait_until="networkidle", timeout=30_000)
        time.sleep(2)

        # 尝试找用户名/密码输入框(Nedev 常见选择器)
        username_selectors = [
            'input[name="username"]',
            'input[name="userid"]',
            'input[name="account"]',
            'input[type="text"]',
            '#username',
            '#userid',
        ]
        password_selectors = [
            'input[name="password"]',
            'input[type="password"]',
            '#password',
        ]
        submit_selectors = [
            'button[type="submit"]',
            'button.login-btn',
            'button.el-button--primary',
            'input[type="submit"]',
        ]

        # 填用户名
        for sel in username_selectors:
            el = page.query_selector(sel)
            if el and el.is_visible():
                el.fill(self.account)
                break

        # 填密码
        for sel in password_selectors:
            el = page.query_selector(sel)
            if el and el.is_visible():
                el.fill(self.password)
                break

        time.sleep(0.5)

        # 点登录
        for sel in submit_selectors:
            el = page.query_selector(sel)
            if el and el.is_visible():
                el.click()
                break

        # 等待跳转
        time.sleep(5)

        # 检查是否登录成功
        # Nedev 5.0 是 Vue hash SPA, 登录成功后 URL 可能仍含 #Login，
        # 不能仅靠 URL 判断。按优先级检测:
        #   1) session cookie  2) localStorage token  3) 页面 UI  4) URL 兜底
        logged_in = False

        # ① 检查 session cookie
        try:
            cookies = self._context.cookies()
            cookie_names = {c['name'] for c in cookies}
            if '.Nedev5.0.Session' in cookie_names or 'nedev_session' in cookie_names:
                logged_in = True
                self.report('pw_login', 10, '检测到 session cookie')
        except Exception:
            cookies = []

        # ② 检查 localStorage 中的 token
        try:
            token = page.evaluate(
                """() => {
                    for (const k of ['Default.AccessToken','nedev_access_token',
                                     'token','access_token','NedevToken','Authorization']) {
                        const v = localStorage.getItem(k);
                        if (v) return v;
                    }
                    return '';
                }"""
            )
            if token:
                self._token = token
                if self._http:
                    self._http.headers['Authorization'] = f'Bearer {token}'
                logged_in = True
                self.report('pw_login', 10, '检测到 localStorage token')
        except Exception:
            pass

        # ③ 检查页面是否出现已登录 UI 元素（用户头像/菜单等）
        if not logged_in:
            user_selectors = [
                '.el-avatar',
                '.user-avatar',
                '.avatar',
                '.el-dropdown',
                '.user-info',
                '.el-menu',
                '[class*="avatar"]',
                '[class*="user-info"]',
                '[class*="header-user"]',
            ]
            for sel in user_selectors:
                el = page.query_selector(sel)
                if el and el.is_visible():
                    logged_in = True
                    self.report('pw_login', 10, f'检测到用户界面元素 {sel}')
                    break

        # ④ URL 兜底: 检查 hash 是否不含 login 相关路径
        if not logged_in:
            current_url = page.url.lower()
            if 'login' not in current_url and 'signin' not in current_url:
                logged_in = True

        # ⑤ 最终兜底: 检查页面可见文本是否包含主界面关键词
        if not logged_in:
            try:
                body_text = page.evaluate(
                    """() => {
                        const el = document.querySelector(
                            '.el-main, .app-main, #app, .layout-container, main'
                        );
                        return el ? el.innerText.slice(0, 2000) : '';
                    }"""
                )
                dashboard_kw = ['工作台','首页','控制台','dashboard','home',
                                '欢迎','我的','待办']
                if any(kw in body_text.lower() for kw in dashboard_kw):
                    logged_in = True
                    self.report('pw_login', 10, '检测到主界面关键词')
            except Exception:
                pass

        # 保存 cookies
        try:
            self._save_cookies(cookies if cookies else self._context.cookies())
        except Exception:
            pass

        if logged_in:
            self.report('pw_login_ok', 15, '浏览器登录成功')
            return True

        self.report('failed', 0, '登录失败,请检查账号密码')
        return False

    # ─────────── OA 立案 ───────────

    def run_filing(self, case_data: dict[str, Any]) -> OAResult:
        """把案件数据推送到 OA 登记接口。

        case_data 结构:
          case_name, case_no, cause, court, parties[], amount, ...
        """
        if self._agent_api_ready:
            return self._run_filing_via_agent_api(case_data)

        self.report("filing_start", 20, "开始 OA 立案...")

        if not self._page:
            self._ensure_browser(headless=True)

        page = self._page
        assert page is not None

        # 导航到 OA 立案页面(Nedev 常见路径)
        filing_paths = [
            "/Nedev/#/case/create",
            "/Nedev/#/project/create",
            "/Nedev/#/business/create",
            "/case/create",
        ]

        navigated = False
        for path in filing_paths:
            try:
                page.goto(f"{self.site_url}{path}", wait_until="networkidle", timeout=15_000)
                time.sleep(2)
                # 检查是否有表单元素
                form = page.query_selector("form, .el-form, [class*='form']")
                if form:
                    navigated = True
                    break
            except Exception:
                continue

        if not navigated:
            self.report("failed", 0, "未找到 OA 立案页面")
            return OAResult(success=False, message="未找到 OA 立案页面")

        self.report("filing_form", 40, "找到立案表单,正在填写...")

        # 自动填写表单(按字段映射)
        filled = 0
        mapping = case_data.get("field_mapping", {})

        # 通用字段填充策略
        field_map = {
            "case_name": case_data.get("name", ""),
            "case_no": case_data.get("case_no", ""),
            "cause": case_data.get("cause", ""),
            "court": case_data.get("court", ""),
            "amount": str(case_data.get("claim_amount", "")),
        }

        # 应用用户自定义映射
        for sys_field, oa_field in mapping.items():
            if sys_field in field_map and field_map[sys_field]:
                try:
                    # 尝试多种选择器
                    selectors = [
                        f'input[name="{oa_field}"]',
                        f'input[placeholder*="{oa_field}"]',
                        f'textarea[name="{oa_field}"]',
                    ]
                    for sel in selectors:
                        el = page.query_selector(sel)
                        if el:
                            el.fill(str(field_map[sys_field]))
                            filled += 1
                            break
                except Exception:
                    pass

        self.report("filing_filled", 70, f"已填写 {filled} 个字段")

        # 尝试点提交
        submit_selectors = [
            'button[type="submit"]',
            'button.el-button--primary',
            'button:has-text("提交")',
            'button:has-text("保存")',
        ]
        for sel in submit_selectors:
            try:
                el = page.query_selector(sel)
                if el and el.is_visible():
                    el.click()
                    time.sleep(3)
                    break
            except Exception:
                continue

        self.report("completed", 100, "OA 立案操作完成")
        return OAResult(success=True, message="OA 立案操作完成", data={"filled_fields": filled})

    def _run_filing_via_agent_api(self, case_data: dict[str, Any]) -> OAResult:
        self.report("filing_start", 20, "开始通过 AgentAPI 登记案件...")
        payload = self._build_case_registration_payload(case_data)
        self.report("filing_submit", 75, "正在提交案件登记...")
        result = self._agent_post("CaseRegistration", {"data": payload})
        self._repair_registered_instance_role_names(result, payload)
        self._repair_registered_proxy_permission(result, payload)
        self._verify_registered_case(result, payload)
        self.report("completed", 100, "OA 立案登记完成", {"result": result})
        return OAResult(success=True, message="OA 立案登记完成", data={"result": result})

    def _build_case_registration_payload(self, case_data: dict[str, Any]) -> dict[str, Any]:
        self.report("filing_prepare", 25, "正在读取 OA 账号信息...")
        profile = self._agent_get("GetAgentProfile")
        self.report("filing_prepare", 30, "正在读取 OA 基础字典...")

        base_type = self._pick_case_type(case_data)
        base_type_id = base_type.get("id")
        categories = self._agent_get("GetCaseCategories", baseTypeId=base_type_id) or []
        category = self._pick_case_category(categories, case_data)
        category_id = category["id"]

        self.report("filing_prepare", 40, "正在匹配案由、审级和代理方...")
        case_head = self._pick_case_head(category, case_data, base_type_id)
        instances = self._agent_get("GetInstances", baseTypeId=base_type_id) or []
        instance = self._pick_instance(instances, case_data)
        instance_role = self._pick_instance_role(instance, case_data)

        self.report("filing_prepare", 55, "正在匹配经办律师、收费方式和地区...")
        employees = self._agent_get("GetEmployees", includeDimission=False) or []
        handling_lawyer_ids = self._pick_employee_ids(employees, case_data)
        identity_type = self._pick_identity_type()
        charge_method = self._pick_charge_method(case_data)
        ajxz = self._pick_optional(self._agent_get("GetAJXZ") or [])
        area = self._pick_default(self._agent_get("GetAreas") or [])

        clients = self._build_clients(case_data, identity_type, instance_role)
        employee_rows = [
            {"EmployeeId": employee_id, "Type": 0, "Sort": 0}
            for employee_id in handling_lawyer_ids
        ]
        self._add_required_source_people(employee_rows, profile, handling_lawyer_ids[0])
        self._validate_required_filing_fields(
            case_data=case_data,
            base_type=base_type,
            category=category,
            case_head=case_head,
            instance=instance,
            instance_role=instance_role,
            clients=clients,
            employees=employee_rows,
            charge_method=charge_method,
            profile=profile,
        )

        data: dict[str, Any] = {
            "CaseCategoryId": category_id,
            "ShouliDate": self._date_value(
                case_data.get("agg_filed_at")
                or case_data.get("filed_at")
                or case_data.get("shouli_date")
            ),
            "clients": clients,
            "employees": employee_rows,
            "ChargeMethodId": charge_method["id"],
            "ChargeAmount": self._number_or_none(case_data.get("charge_amount")),
            "OtherCharge": self._number_or_default(case_data.get("other_charge"), 0),
            "CaseSummary": self._text(
                case_data.get("case_summary")
                or case_data.get("summary")
                or case_data.get("name")
                or "案件登记"
            ),
            "CaseMemo": self._text(case_data.get("case_memo") or case_data.get("note")),
        }

        cause = self._text(case_data.get("agg_cause") or case_data.get("cause"))
        if case_head:
            data["CaseHeadId"] = case_head.get("id")
        elif cause:
            data["CauseAction"] = cause

        if instance:
            data["instanceIds"] = [instance["id"]]
            data["CurrentInstanceId"] = instance["id"]
            if instance_role:
                data["CurrentInstanceRoleId"] = instance_role["id"]
                instance_name_rows = self._build_instance_name_rows(instance, instance_role, clients)
                if instance_name_rows:
                    data["instanceNameSet"] = instance_name_rows
                    data["instanceNames"] = instance_name_rows
                    data["InstanceNames"] = instance_name_rows
                    data["instances"] = [
                        {
                            "InstanceId": instance["id"],
                            "IsCurrent": True,
                            "roles": [
                                {
                                    "RoleId": row["InstanceRoleId"],
                                    "Names": row.get("Names") or "",
                                }
                                for row in instance_name_rows
                            ],
                        }
                    ]

        if ajxz:
            data["AJXZId"] = ajxz.get("id")
        if area:
            data["AreaId"] = area.get("id")

        amount = self._number_or_none(case_data.get("agg_claim_amount") or case_data.get("claim_amount"))
        if base_type.get("hasBiaodi") or amount is not None:
            data["Biaodi"] = amount or 0

        court = self._text(case_data.get("agg_court") or case_data.get("court"))
        if court:
            data["ShouliType"] = 3
            data["Fayuan"] = court

        base_type_name = self._base_type_name(base_type)
        if "民事" in base_type_name or "行政" in base_type_name:
            wtqx_type, wtqx_content = self._proxy_permission(case_data, base_type)
            data["WTQXType"] = wtqx_type
            if wtqx_content:
                data["WTQXContent"] = wtqx_content

        return {k: v for k, v in data.items() if v is not None and v != ""}

    def _validate_required_filing_fields(
        self,
        *,
        case_data: dict[str, Any],
        base_type: dict[str, Any],
        category: dict[str, Any],
        case_head: dict[str, Any] | None,
        instance: dict[str, Any] | None,
        instance_role: dict[str, Any] | None,
        clients: list[dict[str, Any]],
        employees: list[dict[str, Any]],
        charge_method: dict[str, Any],
        profile: dict[str, Any],
    ) -> None:
        missing: list[str] = []
        base_type_name = self._base_type_name(base_type)
        cause = self._text(case_data.get("agg_cause") or case_data.get("cause"))
        shouli_date = self._explicit_date_value(
            case_data.get("agg_filed_at")
            or case_data.get("filed_at")
            or case_data.get("shouli_date")
        )
        claim_amount = self._number_or_none(case_data.get("agg_claim_amount") or case_data.get("claim_amount"))

        if not category.get("id"):
            missing.append("案件分类")
        if not self._text(case_data.get("baseTypeName") or case_data.get("case_type") or case_data.get("type")):
            missing.append("案件基础类型")
        if not self._text(
            case_data.get("caseCategoryName")
            or case_data.get("case_category")
            or case_data.get("category")
        ):
            missing.append("案件分类")
        if not case_head and not cause:
            missing.append("案由")
        if not clients:
            missing.append("委托人")
        if not any(row.get("RoleType") == 0 for row in clients):
            missing.append("委托人")
        if self._requires_opponent(base_type, case_data) and not any(row.get("RoleType") == 1 for row in clients):
            missing.append("对方")
        if not self._lawyer_names(case_data):
            missing.append("经办律师")
        if not any(row.get("Type") == 0 for row in employees):
            missing.append("经办律师")
        if profile.get("sourcePerson1Required") and not any(row.get("Type") == 9 for row in employees):
            missing.append("案源人")
        if profile.get("sourcePerson2Required") and not any(row.get("Type") == 10 for row in employees):
            missing.append("案源人2")
        if not self._text(
            case_data.get("charge_method")
            or case_data.get("charge_method_name")
            or case_data.get("ChargeMethodName")
        ):
            missing.append("收费方式")
        if self._number_or_none(case_data.get("charge_amount")) is None:
            missing.append("委托费用")
        if base_type.get("hasBiaodi") and claim_amount is None:
            missing.append("标的额")
        if not self._text(case_data.get("proxy_permission") or case_data.get("WTQXContent")):
            missing.append("代理权限(一般代理/特别授权)")
        if ("民事" in base_type_name or "行政" in base_type_name) and not self._text(
            case_data.get("proxy_permission") or case_data.get("WTQXContent")
        ):
            missing.append("代理权限(一般代理/特别授权)")
        if instance and not instance_role:
            missing.append("代理方")
        if (charge_method.get("name") or "").find("风险") >= 0 and not self._text(case_data.get("charge_memo")):
            missing.append("风险收费说明")
        if (charge_method.get("name") or "") == "另案已收" and not self._text(case_data.get("source_charged_case_id")):
            missing.append("另案已收案件")
        shouli_type = self._text(case_data.get("shouli_type"))
        if (shouli_type == "3" or self._text(case_data.get("agg_court") or case_data.get("court"))) and not self._text(
            case_data.get("agg_court") or case_data.get("court")
        ):
            missing.append("受理法院")
        if missing:
            seen: list[str] = []
            for item in missing:
                if item not in seen:
                    seen.append(item)
            raise RuntimeError(
                "OA 立案信息未确认: "
                + "、".join(seen)
                + "。请补齐后再提交，系统不会自动默认这些字段。"
            )

    def _requires_opponent(self, base_type: dict[str, Any], case_data: dict[str, Any]) -> bool:
        base_type_name = self._base_type_name(base_type)
        base_type_enum = self._text(base_type.get("baseTypeEnum"))
        return (
            "民事" in base_type_name
            or "民事" in base_type_enum
            or "执行" in base_type_name
            or "行政" in base_type_name
            or case_data.get("XS_FJMS") is True
        )

    def _pick_case_category(
        self,
        categories: list[dict[str, Any]],
        case_data: dict[str, Any],
    ) -> dict[str, Any]:
        wanted = self._text(
            case_data.get("caseCategoryName")
            or case_data.get("case_category")
            or case_data.get("category")
        )
        cause = self._text(case_data.get("agg_cause") or case_data.get("cause"))
        for item in categories:
            name = self._text(item.get("name"))
            if wanted and (wanted in name or name in wanted):
                return item
        for item in categories:
            name = self._text(item.get("name"))
            key = self._text(item.get("defaultCaseHeadKey"))
            if cause and ((key and key in cause) or (name and name.replace("、准", "")[:2] in cause)):
                return item
        if "合同" in cause:
            for item in categories:
                if "合同" in self._text(item.get("name")):
                    return item
        return self._pick_first(categories, "案件分类")

    def _pick_case_type(self, case_data: dict[str, Any]) -> dict[str, Any]:
        types = self._agent_get("GetCaseTypes") or []
        wanted = self._text(case_data.get("baseTypeName") or case_data.get("case_type") or case_data.get("type"))
        if not wanted:
            raise RuntimeError("OA 立案信息未确认: 案件基础类型。请补齐后再提交，系统不会自动默认这些字段。")
        for item in types:
            name = self._base_type_name(item)
            if wanted and (wanted in name or name in wanted):
                return item
        raise RuntimeError(f"OA 未找到案件基础类型: {wanted}")

    def _pick_case_head(
        self,
        category: dict[str, Any],
        case_data: dict[str, Any],
        base_type_id: int | None,
    ) -> dict[str, Any] | None:
        cause = self._text(case_data.get("agg_cause") or case_data.get("cause"))
        if cause:
            heads = self._agent_get(
                "GetCaseHeads",
                caseCategoryId=category.get("id"),
                keyword=cause,
                leafOnly=True,
            ) or []
            if heads:
                return self._best_case_head_match(heads, cause)
            heads = self._agent_get(
                "GetCaseHeads",
                baseTypeId=base_type_id,
                keyword=cause,
                leafOnly=True,
            ) or []
            if heads:
                return self._best_case_head_match(heads, cause)
            self.report(
                "case_head_fallback",
                35,
                f"OA 案由字典未返回匹配项，改用案由文本: {cause}",
            )
            return None

        default_id = category.get("defaultCaseHeadId")
        if default_id:
            return {"id": default_id, "name": category.get("defaultCaseHeadName")}
        return None

    def _best_case_head_match(self, heads: list[dict[str, Any]], cause: str) -> dict[str, Any]:
        for item in heads:
            name = self._text(item.get("name"))
            if name == cause:
                return item
        for item in heads:
            name = self._text(item.get("name"))
            if name and (name in cause or cause in name):
                return item
        return self._pick_first(heads, "案由")

    def _pick_employee_ids(
        self,
        employees: list[dict[str, Any]],
        case_data: dict[str, Any],
    ) -> list[int]:
        names = self._lawyer_names(case_data)
        ids: list[int] = []
        missing: list[str] = []
        for name in names:
            hit = None
            for item in employees:
                display = self._text(item.get("displayName") or item.get("name"))
                if name == display or name in display or display in name:
                    hit = item
                    break
            if hit:
                ids.append(int(hit["id"]))
            else:
                missing.append(name)
        if missing:
            raise RuntimeError(f"OA 未找到经办律师: {'、'.join(missing)}")
        return ids

    def _lawyer_names(self, case_data: dict[str, Any]) -> list[str]:
        raw = (
            case_data.get("handling_lawyers")
            or case_data.get("lawyer_names")
            or case_data.get("employee_names")
            or case_data.get("empNames")
            or case_data.get("lawyer")
            or case_data.get("lawyer_name")
            or case_data.get("employee_name")
        )
        if isinstance(raw, list):
            return [self._text(x) for x in raw if self._text(x)]
        text = self._text(raw)
        if not text:
            return []
        return [
            part.strip()
            for part in text.replace(",", "、").replace("，", "、").split("、")
            if part.strip()
        ]

    def _proxy_permission(self, case_data: dict[str, Any], base_type: dict[str, Any]) -> tuple[int, str]:
        text = self._text(case_data.get("proxy_permission") or case_data.get("WTQXContent"))
        if not text:
            raise RuntimeError("缺少代理权限")
        if "特" in text:
            wtqx_type = 2
        else:
            wtqx_type = 1
        return wtqx_type, self._text(case_data.get("WTQXContent")) or self._default_wtqx_content(base_type, wtqx_type)

    def _pick_identity_type(self) -> dict[str, Any]:
        identity_types = self._agent_get("GetIdentityTypes") or []
        for item in identity_types:
            if "自然" in self._text(item.get("name")):
                return item
        return self._pick_first(identity_types, "身份类型")

    def _pick_charge_method(self, case_data: dict[str, Any]) -> dict[str, Any]:
        methods = self._agent_get("GetChargeMethods", includeDisabled=False) or []
        wanted = self._text(
            case_data.get("charge_method")
            or case_data.get("charge_method_name")
            or case_data.get("ChargeMethodName")
        )
        if not wanted:
            raise RuntimeError("OA 立案信息未确认: 收费方式。请补齐后再提交，系统不会自动默认这些字段。")
        for item in methods:
            name = self._text(item.get("name") or item.get("baseChargeMethod"))
            if wanted == name or wanted in name or name in wanted:
                return item
        raise RuntimeError(f"OA 未找到收费方式: {wanted}")

    def _pick_charge_method_legacy(self) -> dict[str, Any]:
        methods = self._agent_get("GetChargeMethods", includeDisabled=False) or []
        for item in methods:
            name = self._text(item.get("name") or item.get("baseChargeMethod"))
            if "固定" in name or "普通" in name or "计件" in name:
                return item
        return self._pick_first(methods, "收费方式")

    def _build_clients(
        self,
        case_data: dict[str, Any],
        identity_type: dict[str, Any],
        instance_role: dict[str, Any] | None,
    ) -> list[dict[str, Any]]:
        identity_id = identity_type["id"]
        clients = self._clients_from_litigation_roles(case_data, identity_id, instance_role)
        if clients:
            return clients
        clients = self._clients_from_party_contacts(case_data, identity_id)
        if clients:
            return clients

        plaintiffs = self._party_names(case_data, "plaintiffs", "agg_plaintiffs")
        defendants = self._party_names(case_data, "defendants", "agg_defendants")
        client_name = self._text(
            case_data.get("client_name")
            or case_data.get("party_name")
            or (plaintiffs[0] if plaintiffs else "")
        )
        opponent_name = self._text(case_data.get("opponent_name") or (defendants[0] if defendants else ""))
        rows: list[dict[str, Any]] = []
        if client_name:
            rows.append({"ClientId": None, "RoleType": 0, "IdentityTypeId": identity_id, "Name": client_name})
        if opponent_name:
            rows.append({"ClientId": None, "RoleType": 1, "IdentityTypeId": identity_id, "Name": opponent_name})
        if not rows:
            raise RuntimeError("本机案件缺少委托人信息，无法登记 OA 案件")
        if not any(row["RoleType"] == 0 for row in rows):
            rows[0]["RoleType"] = 0
        return rows

    def _clients_from_litigation_roles(
        self,
        case_data: dict[str, Any],
        identity_id: int,
        instance_role: dict[str, Any] | None,
    ) -> list[dict[str, Any]]:
        plaintiffs = self._party_names(case_data, "plaintiffs", "agg_plaintiffs")
        defendants = self._party_names(case_data, "defendants", "agg_defendants")
        third_parties = self._party_names(case_data, "third_parties", "agg_third_parties")
        if not plaintiffs and not defendants and not third_parties:
            return []

        proxy_side = self._text(
            case_data.get("proxy_side")
            or case_data.get("current_instance_role")
            or case_data.get("instance_role")
            or case_data.get("agg_our_side")
            or case_data.get("our_side")
            or (instance_role or {}).get("name")
        )

        rows: list[dict[str, Any]] = []
        if "被告" in proxy_side:
            rows.extend(self._client_rows(defendants, 0, identity_id, case_data))
            rows.extend(self._client_rows(plaintiffs, 1, identity_id, case_data))
        elif "第三" in proxy_side:
            rows.extend(self._client_rows(third_parties, 0, identity_id, case_data))
            rows.extend(self._client_rows(plaintiffs, 1, identity_id, case_data))
            rows.extend(self._client_rows(defendants, 1, identity_id, case_data))
        else:
            rows.extend(self._client_rows(plaintiffs, 0, identity_id, case_data))
            rows.extend(self._client_rows(defendants, 1, identity_id, case_data))
            rows.extend(self._client_rows(third_parties, 2, identity_id, case_data))
        return rows

    def _client_rows(
        self,
        names: list[str],
        role_type: int,
        identity_id: int,
        case_data: dict[str, Any],
    ) -> list[dict[str, Any]]:
        contact_map = self._party_contact_map(case_data)
        rows: list[dict[str, Any]] = []
        for name in names:
            contact = contact_map.get(name) or {}
            rows.append(
                {
                    "ClientId": None,
                    "RoleType": role_type,
                    "IdentityTypeId": identity_id,
                    "Name": name,
                    "Mobile": self._text(contact.get("phone") or contact.get("mobile")) or None,
                    "Email": self._text(contact.get("email")) or None,
                }
            )
        return rows

    def _clients_from_party_contacts(self, case_data: dict[str, Any], identity_id: int) -> list[dict[str, Any]]:
        raw = case_data.get("agg_party_contacts") or case_data.get("party_contacts")
        if not raw:
            return []
        try:
            items = json.loads(raw) if isinstance(raw, str) else raw
        except Exception:
            return []
        if not isinstance(items, list):
            return []

        rows: list[dict[str, Any]] = []
        for item in items:
            if not isinstance(item, dict):
                continue
            name = self._text(item.get("party") or item.get("name"))
            if not name:
                continue
            role = self._text(item.get("role"))
            is_our_side = item.get("is_our_side")
            role_type = (
                0
                if is_our_side is True
                else 1
                if is_our_side is False or "被告" in role or "对方" in role
                else 8
            )
            rows.append({
                "ClientId": None,
                "RoleType": role_type,
                "IdentityTypeId": identity_id,
                "Name": name,
                "Mobile": self._text(item.get("phone")) or None,
                "Email": self._text(item.get("email")) or None,
            })
        if rows and not any(row["RoleType"] == 0 for row in rows):
            rows[0]["RoleType"] = 0
        return rows

    def _party_names(self, case_data: dict[str, Any], plain_key: str, agg_key: str) -> list[str]:
        raw = case_data.get(plain_key)
        if isinstance(raw, list):
            return [self._text(x) for x in raw if self._text(x)]
        names = self._json_names(raw)
        if names:
            return names
        return self._json_names(case_data.get(agg_key))

    def _party_contact_map(self, case_data: dict[str, Any]) -> dict[str, dict[str, Any]]:
        raw = case_data.get("agg_party_contacts") or case_data.get("party_contacts")
        if not raw:
            return {}
        try:
            items = json.loads(raw) if isinstance(raw, str) else raw
        except Exception:
            return {}
        if not isinstance(items, list):
            return {}
        contacts: dict[str, dict[str, Any]] = {}
        for item in items:
            if not isinstance(item, dict):
                continue
            name = self._text(item.get("party") or item.get("name"))
            if name:
                contacts[name] = item
        return contacts

    def _add_required_source_people(
        self,
        employees: list[dict[str, Any]],
        profile: dict[str, Any],
        fallback_employee_id: int,
    ) -> None:
        if profile.get("sourcePerson1Required"):
            employees.append({"EmployeeId": fallback_employee_id, "Type": 9, "Sort": 0})
        if profile.get("sourcePerson2Required"):
            employees.append({"EmployeeId": fallback_employee_id, "Type": 10, "Sort": 0})

    def _build_instance_name_rows(
        self,
        instance: dict[str, Any],
        instance_role: dict[str, Any],
        clients: list[dict[str, Any]],
    ) -> list[dict[str, Any]]:
        roles = sorted(instance.get("roles") or [], key=lambda row: row.get("sort") or 0)
        if not roles:
            return []

        our_names = self._join_client_names(clients, 0)
        opponent_names = self._join_client_names(clients, 1)
        third_names = self._join_client_names(clients, 2) or self._join_client_names(clients, 8)
        current_role_id = instance_role.get("id")
        current_sort = instance_role.get("sort")

        rows: list[dict[str, Any]] = []
        for role in roles:
            role_id = role.get("id")
            sort = role.get("sort")
            name = self._text(role.get("name"))
            names = ""
            if role_id == current_role_id:
                names = our_names or third_names
            elif current_sort == 0 and sort == 1:
                names = opponent_names
            elif current_sort == 1 and sort == 0:
                names = opponent_names
            elif "原告" in name and current_sort != 0:
                names = opponent_names
            elif ("被告" in name or "对方" in name) and current_sort != 1:
                names = opponent_names
            elif "第三" in name:
                names = third_names

            rows.append(
                {
                    "CurrentInstanceId": instance["id"],
                    "InstanceId": instance["id"],
                    "CurrentInstance": instance["id"],
                    "InstanceRoleId": role_id,
                    "RoleId": role_id,
                    "InstanceRole": role_id,
                    "Names": names,
                }
            )
        return rows

    def _join_client_names(self, clients: list[dict[str, Any]], role_type: int) -> str:
        return "、".join(
            self._text(row.get("Name") or row.get("name"))
            for row in clients
            if row.get("RoleType") == role_type and self._text(row.get("Name") or row.get("name"))
        )

    def _verify_registered_case(self, result: Any, payload: dict[str, Any]) -> None:
        lawcase_id = self._extract_lawcase_id(result)
        expected_rows = payload.get("instanceNameSet") or []
        if not lawcase_id or not expected_rows:
            return

        self.report("filing_verify", 90, "正在复查 OA 受理信息...")
        detail = self._agent_get("GetLawcaseDetail", lawcaseId=lawcase_id)
        instances = detail.get("instances") if isinstance(detail, dict) else None
        if not isinstance(instances, list):
            return

        expected_by_role = {
            row.get("InstanceRoleId"): self._text(row.get("Names"))
            for row in expected_rows
            if self._text(row.get("Names"))
        }
        if not expected_by_role:
            return

        actual_by_role: dict[int, str] = {}
        for instance in instances:
            for role in instance.get("roles") or []:
                role_id = role.get("roleId") or role.get("id") or role.get("RoleId")
                if role_id is not None:
                    actual_by_role[int(role_id)] = self._text(role.get("names") or role.get("Names"))

        missing = [
            names
            for role_id, names in expected_by_role.items()
            if self._text(actual_by_role.get(int(role_id))) != names
        ]
        if missing:
            raise RuntimeError(
                "OA 已返回案件号，但受理信息中的原告/被告未写入成功；"
                "系统已拦截为失败，请不要按成功案件处理。缺失: " + "、".join(missing)
            )

    def _repair_registered_instance_role_names(self, result: Any, payload: dict[str, Any]) -> None:
        lawcase_id = self._extract_lawcase_id(result)
        expected_rows = payload.get("instanceNameSet") or []
        if not lawcase_id or not expected_rows:
            return

        expected_by_role = {
            int(row["InstanceRoleId"]): self._text(row.get("Names"))
            for row in expected_rows
            if row.get("InstanceRoleId") is not None and self._text(row.get("Names"))
        }
        if not expected_by_role:
            return

        self.report("filing_repair", 88, "正在补齐 OA 受理信息中的原告/被告名称...")
        detail = self._agent_get("GetLawcaseDetail", lawcaseId=lawcase_id)
        modified_entities = self._build_instance_role_name_updates(detail, expected_by_role)
        if not modified_entities:
            return

        change_set = {"ApplicationData": {"ModifiedEntities": modified_entities}}
        errors: list[str] = []
        for owner in ("Page:LawcaseDetails_民事@1", "Page:LawcaseDetails@1"):
            try:
                self._save_changes_v2(change_set, [owner])
                return
            except Exception as exc:
                errors.append(f"{owner}: {exc}")
        raise RuntimeError("OA 受理信息补写失败: " + "；".join(errors))

    def _repair_registered_proxy_permission(self, result: Any, payload: dict[str, Any]) -> None:
        lawcase_id = self._extract_lawcase_id(result)
        expected_type = payload.get("WTQXType")
        expected_content = self._text(payload.get("WTQXContent"))
        if not lawcase_id or expected_type is None:
            return

        detail = self._get_entity("ApplicationData.Lawcase", [lawcase_id], ["Page:LawcaseDetails_民事@1"])
        actual_type = detail.get("WTQXType") if isinstance(detail, dict) else None
        actual_content = self._text(detail.get("WTQXContent")) if isinstance(detail, dict) else ""
        if actual_type == expected_type and (not expected_content or actual_content == expected_content):
            return

        self.report("filing_repair", 89, "正在补齐 OA 代理权限和代理事项...")
        modified: dict[str, Any] = {
            "__EntityType": "ApplicationData.Lawcase",
            "__UniqueId": 1,
            "Id": int(lawcase_id),
            "WTQXType": expected_type,
            "WTQXType_IsChanged": True,
        }
        if expected_content:
            modified["WTQXContent"] = expected_content
            modified["WTQXContent_IsChanged"] = True

        change_set = {"ApplicationData": {"ModifiedEntities": [modified]}}
        errors: list[str] = []
        for owner in ("Page:LawcaseDetails_民事@1", "Page:LawcaseDetails@1"):
            try:
                self._save_changes_v2(change_set, [owner])
                return
            except Exception as exc:
                errors.append(f"{owner}: {exc}")
        raise RuntimeError("OA 代理事项补写失败: " + "；".join(errors))

    def _build_instance_role_name_updates(
        self,
        detail: Any,
        expected_by_role: dict[int, str],
    ) -> list[dict[str, Any]]:
        instances = detail.get("instances") if isinstance(detail, dict) else None
        if not isinstance(instances, list):
            return []

        modified_entities: list[dict[str, Any]] = []
        unique_id = 1
        for instance in instances:
            for role in instance.get("roles") or []:
                role_id = role.get("roleId") or role.get("RoleId")
                row_id = role.get("id") or role.get("Id")
                if role_id is None or row_id is None:
                    continue
                expected = expected_by_role.get(int(role_id))
                if not expected:
                    continue
                actual = self._text(role.get("names") or role.get("Names"))
                if actual == expected:
                    continue
                modified_entities.append(
                    {
                        "__EntityType": "ApplicationData.LawcaseInstanceClient",
                        "__UniqueId": unique_id,
                        "Id": int(row_id),
                        "Names": expected,
                        "Names_IsChanged": True,
                    }
                )
                unique_id += 1
        return modified_entities

    def _save_changes_v2(self, change_set: dict[str, Any], owners: list[str]) -> Any:
        if not self._http or not self._agent_api_ready:
            raise RuntimeError("AgentAPI 未登录")

        resp = self._http.post(
            f"{self.site_url}/DataService/SaveChangesV2",
            files={
                "owners": (None, json.dumps(owners, ensure_ascii=False)),
                "changeSet": (None, json.dumps(change_set, ensure_ascii=False)),
            },
        )
        try:
            data = resp.json()
        except Exception as exc:
            raise RuntimeError(f"SaveChangesV2 返回非 JSON: HTTP {resp.status_code} {resp.text[:200]}") from exc

        if resp.status_code != 200:
            raise RuntimeError(data if isinstance(data, str) else f"HTTP {resp.status_code}")
        if data.get("Status") == "OK":
            return data
        raise RuntimeError(self._text(data.get("ErrorMessage") or data.get("Status") or data))

    def _get_entity(self, entity_type: str, keys: list[Any], owners: list[str]) -> dict[str, Any] | None:
        if not self._http or not self._agent_api_ready:
            raise RuntimeError("AgentAPI 未登录")

        resp = self._http.post(
            f"{self.site_url}/DataService/GetEntity",
            data={
                "entityType": entity_type,
                "entityKeys": json.dumps(keys, ensure_ascii=False),
                "owners": json.dumps(owners, ensure_ascii=False),
            },
        )
        try:
            data = resp.json()
        except Exception as exc:
            raise RuntimeError(f"GetEntity 返回非 JSON: HTTP {resp.status_code} {resp.text[:200]}") from exc
        if resp.status_code != 200:
            raise RuntimeError(f"GetEntity HTTP {resp.status_code}")
        if data.get("Status") == "OK":
            return data.get("Value")
        raise RuntimeError(self._text(data.get("ErrorMessage") or data.get("Status") or data))

    def _default_wtqx_content(self, base_type: dict[str, Any], wtqx_type: int) -> str:
        base_type_id = base_type.get("id")
        base_type_name = self._base_type_name(base_type)
        candidates: list[str] = []
        if "民事" in base_type_name:
            candidates.append("民事特别授权" if wtqx_type == 2 else "民事一般代理")
        if "行政" in base_type_name:
            candidates.append("行政特别授权" if wtqx_type == 2 else "行政一般代理")
        candidates.append("特别授权" if wtqx_type == 2 else "一般代理")

        for keyword in candidates:
            item = self._find_entity("ApplicationData.Base_WTQX", keyword, ["Page:LawcaseDetails_民事@1"])
            if not isinstance(item, dict):
                continue
            if base_type_id is not None and item.get("baseType") not in (None, base_type_id):
                continue
            if item.get("ForType") not in (None, wtqx_type):
                continue
            content = self._text(item.get("Content"))
            if content:
                return content
        return ""

    def _find_entity(self, entity_type: str, search_key: str, owners: list[str]) -> Any:
        if not self._http or not self._agent_api_ready:
            raise RuntimeError("AgentAPI 未登录")

        resp = self._http.post(
            f"{self.site_url}/DataService/FindEntity",
            data={
                "owners": json.dumps(owners, ensure_ascii=False),
                "entityType": entity_type,
                "searchKey": search_key,
            },
        )
        try:
            data = resp.json()
        except Exception:
            return None
        if resp.status_code != 200 or data.get("Status") != "OK":
            return None
        return data.get("Value")

    def _extract_lawcase_id(self, result: Any) -> int | None:
        if not isinstance(result, dict):
            return None
        for key in ("lawCaseId", "lawcaseId", "caseId", "id", "Id"):
            value = result.get(key)
            if value is None:
                continue
            try:
                return int(value)
            except (TypeError, ValueError):
                continue
        return None

    def _pick_instance_role(
        self,
        instance: dict[str, Any] | None,
        case_data: dict[str, Any],
    ) -> dict[str, Any] | None:
        if not instance:
            return None
        roles = instance.get("roles") or []
        if not roles:
            return None
        wanted = self._text(
            case_data.get("current_instance_role")
            or case_data.get("instance_role")
            or case_data.get("proxy_side")
            or case_data.get("agg_our_side")
            or case_data.get("our_side")
        )
        if not wanted:
            raise RuntimeError("OA 立案信息未确认: 代理方。请补齐后再提交，系统不会自动默认这些字段。")
        for role in roles:
            name = self._text(role.get("name"))
            if wanted and (wanted in name or name in wanted):
                return role
        raise RuntimeError(f"OA 未找到代理方: {wanted}")

    def _pick_first(self, items: list[dict[str, Any]], label: str) -> dict[str, Any]:
        if not items:
            raise RuntimeError(f"OA 未返回{label}，无法继续登记")
        return items[0]

    def _pick_optional(self, items: list[dict[str, Any]]) -> dict[str, Any] | None:
        return items[0] if items else None

    def _pick_default(self, items: list[dict[str, Any]]) -> dict[str, Any] | None:
        for item in items:
            if item.get("isDefault"):
                return item
        return self._pick_optional(items)

    def _pick_instance(
        self,
        instances: list[dict[str, Any]],
        case_data: dict[str, Any],
    ) -> dict[str, Any] | None:
        if not instances:
            return None
        wanted = self._text(
            case_data.get("current_instance")
            or case_data.get("instance")
            or case_data.get("instance_name")
            or case_data.get("proxy_stage")
        )
        if not wanted:
            raise RuntimeError("OA 立案信息未确认: 代理阶段。请补齐后再提交，系统不会自动默认这些字段。")
        for item in instances:
            name = self._text(item.get("name"))
            if wanted == name or wanted in name or name in wanted:
                return item
        raise RuntimeError(f"OA 未找到代理阶段: {wanted}")

    def _base_type_name(self, item: dict[str, Any]) -> str:
        return self._text(item.get("name") or item.get("baseTypeName") or item.get("baseTypeEnum"))

    def _json_names(self, raw: Any) -> list[str]:
        if not raw:
            return []
        try:
            items = json.loads(raw) if isinstance(raw, str) else raw
        except Exception:
            return []
        if isinstance(items, list):
            return [self._text(x) for x in items if self._text(x)]
        return []

    def _text(self, value: Any) -> str:
        if value is None:
            return ""
        return str(value).strip()

    def _date_value(self, value: Any) -> str:
        explicit = self._explicit_date_value(value)
        return explicit or date.today().isoformat()

    def _explicit_date_value(self, value: Any) -> str:
        text = self._text(value)
        if len(text) >= 10:
            return text[:10]
        return ""

    def _number_or_none(self, value: Any) -> float | None:
        if value is None or value == "":
            return None
        try:
            return float(value)
        except (TypeError, ValueError):
            return None

    def _number_or_default(self, value: Any, default: float) -> float:
        parsed = self._number_or_none(value)
        return default if parsed is None else parsed

    # ─────────── 案件导入 ───────────

    def run_case_import(self) -> OAResult:
        """从 OA 拉取案件列表。"""
        self.report("case_import_start", 20, "开始从 OA 导入案件...")

        if self._agent_api_ready:
            return self._import_cases_via_agent_api()

        # 尝试 API 拉取
        if self._token and self._http:
            return self._import_cases_via_api()
        # 兜底:Playwright
        return self._import_cases_via_playwright()

    def _import_cases_via_api(self) -> OAResult:
        """通过 API 拉取案件列表。"""
        assert self._http is not None

        case_api_paths = [
            "/api/cases",
            "/api/projects",
            "/api/business/list",
            "/Nedev/api/cases",
        ]

        for path in case_api_paths:
            try:
                resp = self._http.get(f"{self.site_url}{path}", params={"page": 1, "pageSize": 100})
                if resp.status_code == 200:
                    data = resp.json()
                    items = data.get("data", {}).get("items", data.get("data", data.get("list", [])))
                    if isinstance(items, list) and items:
                        self.report("completed", 100, f"从 OA 获取到 {len(items)} 条案件", {"cases": items})
                        return OAResult(success=True, message=f"获取到 {len(items)} 条案件", data={"cases": items, "count": len(items)})
            except Exception:
                continue

        self.report("failed", 0, "API 拉取案件失败")
        return OAResult(success=False, message="API 拉取案件失败")

    def _import_cases_via_agent_api(self) -> OAResult:
        page_size = 100
        page_index = 0
        all_items: list[dict[str, Any]] = []
        total = 0

        while True:
            data = self._agent_get("GetLawcases", pageIndex=page_index, pageSize=page_size)
            if not isinstance(data, dict):
                break
            items = data.get("data", [])
            if not isinstance(items, list) or not items:
                break
            total = int(data.get("total") or total or len(items))
            all_items.extend(items)
            pct = min(95, 20 + int((len(all_items) / max(total, 1)) * 70))
            self.report("case_import_page", pct, f"已拉取 {len(all_items)} / {total} 条 OA 案件")
            if len(all_items) >= total or len(items) < page_size:
                break
            page_index += 1

        self.report("completed", 100, f"从 OA 获取到 {len(all_items)} 条案件", {"cases": all_items})
        return OAResult(
            success=True,
            message=f"获取到 {len(all_items)} 条案件",
            data={"cases": all_items, "count": len(all_items), "total": total or len(all_items)},
        )

    def _import_cases_via_playwright(self) -> OAResult:
        """通过 Playwright 抓取案件列表页。"""
        self._ensure_browser(headless=True)
        page = self._page
        assert page is not None

        case_paths = [
            "/Nedev/#/case/list",
            "/Nedev/#/project/list",
            "/case/list",
        ]

        for path in case_paths:
            try:
                page.goto(f"{self.site_url}{path}", wait_until="networkidle", timeout=15_000)
                time.sleep(3)

                # 尝试从表格提取数据(Nedev 用 ElementPlus/AntDesign 表格)
                rows = page.query_selector_all("table tbody tr, .el-table__body tr, .ant-table-row")
                if rows:
                    cases = []
                    for row in rows[:100]:
                        cells = row.query_selector_all("td")
                        if len(cells) >= 2:
                            case = {
                                "name": cells[0].inner_text().strip(),
                                "status": cells[1].inner_text().strip() if len(cells) > 1 else "",
                            }
                            cases.append(case)

                    if cases:
                        self.report("completed", 100, f"从页面抓取到 {len(cases)} 条案件", {"cases": cases})
                        return OAResult(success=True, message=f"抓取到 {len(cases)} 条案件", data={"cases": cases, "count": len(cases)})
            except Exception:
                continue

        self.report("failed", 0, "未找到案件列表页面")
        return OAResult(success=False, message="未找到案件列表页面")

    # ─────────── 客户导入 ───────────

    def run_client_import(self) -> OAResult:
        """从 OA 拉取客户列表。"""
        self.report("client_import_start", 20, "开始从 OA 导入客户...")

        if self._agent_api_ready:
            return OAResult(
                success=False,
                message="AgentAPI 文档未提供客户列表接口；可通过案件详情中的 clients 获取案件相关客户",
            )

        if self._token and self._http:
            return self._import_clients_via_api()
        return self._import_clients_via_playwright()

    def _import_clients_via_api(self) -> OAResult:
        """通过 API 拉取客户列表。"""
        assert self._http is not None

        client_api_paths = [
            "/api/clients",
            "/api/customers",
            "/api/contacts",
            "/Nedev/api/clients",
        ]

        for path in client_api_paths:
            try:
                resp = self._http.get(f"{self.site_url}{path}", params={"page": 1, "pageSize": 500})
                if resp.status_code == 200:
                    data = resp.json()
                    items = data.get("data", {}).get("items", data.get("data", data.get("list", [])))
                    if isinstance(items, list) and items:
                        self.report("completed", 100, f"从 OA 获取到 {len(items)} 个客户", {"clients": items})
                        return OAResult(success=True, message=f"获取到 {len(items)} 个客户", data={"clients": items, "count": len(items)})
            except Exception:
                continue

        self.report("failed", 0, "API 拉取客户失败")
        return OAResult(success=False, message="API 拉取客户失败")

    def _import_clients_via_playwright(self) -> OAResult:
        """通过 Playwright 抓取客户列表页。"""
        self._ensure_browser(headless=True)
        page = self._page
        assert page is not None

        client_paths = [
            "/Nedev/#/client/list",
            "/Nedev/#/customer/list",
            "/Nedev/#/contact/list",
            "/client/list",
        ]

        for path in client_paths:
            try:
                page.goto(f"{self.site_url}{path}", wait_until="networkidle", timeout=15_000)
                time.sleep(3)

                rows = page.query_selector_all("table tbody tr, .el-table__body tr, .ant-table-row")
                if rows:
                    clients = []
                    for row in rows[:500]:
                        cells = row.query_selector_all("td")
                        if len(cells) >= 1:
                            client = {
                                "name": cells[0].inner_text().strip(),
                                "phone": cells[1].inner_text().strip() if len(cells) > 1 else "",
                                "type": cells[2].inner_text().strip() if len(cells) > 2 else "",
                            }
                            clients.append(client)

                    if clients:
                        self.report("completed", 100, f"从页面抓取到 {len(clients)} 个客户", {"clients": clients})
                        return OAResult(success=True, message=f"抓取到 {len(clients)} 个客户", data={"clients": clients, "count": len(clients)})
            except Exception:
                continue

        self.report("failed", 0, "未找到客户列表页面")
        return OAResult(success=False, message="未找到客户列表页面")
