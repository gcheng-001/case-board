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
import re
import sqlite3
import time
import unicodedata
import zipfile
from datetime import date
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any
from urllib.parse import unquote, urljoin

import httpx

from .base import OAResult, OAScriptBase


ACTIVE_CASE_STATUSES = {1, 2, 3, 4, 5}
RISK_CHARGE_METHOD_ID = 5
RISK_PROHIBITED_KEYWORDS = (
    "刑事",
    "行政诉讼",
    "国家赔偿",
    "群体性诉讼",
    "婚姻",
    "离婚",
    "继承",
    "社会保险",
    "最低生活保障",
    "赡养费",
    "抚养费",
    "扶养费",
    "抚恤金",
    "救济金",
    "工伤赔偿",
    "劳动报酬",
)
RISK_FEE_TIERS = (
    (Decimal("1000000"), Decimal("0.18")),
    (Decimal("4000000"), Decimal("0.15")),
    (Decimal("5000000"), Decimal("0.12")),
    (Decimal("40000000"), Decimal("0.09")),
    (None, Decimal("0.06")),
)


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

    def _conflict_search(self, case_data: dict[str, Any]) -> bool | dict[str, Any]:
        """立案前利益冲突检索。

        从 case_data 提取委托人、对方、第三人等当事人名称和证件号，
        拼接 keys 调用 ConflictSearch 接口。
        返回 False 表示无冲突（可继续立案）；dict 表示命中冲突（含详情）。
        """
        # 收集所有当事人名称
        names: list[str] = []
        for key in ("agg_plaintiffs", "agg_defendants", "agg_third_parties",
                     "plaintiffs", "defendants", "third_parties"):
            raw = case_data.get(key)
            if isinstance(raw, list):
                names.extend(str(x) for x in raw)
            elif isinstance(raw, str):
                try:
                    items = json.loads(raw)
                    if isinstance(items, list):
                        names.extend(str(x) for x in items)
                except Exception:
                    pass

        # 从 party_contacts 补充当事人
        contacts = case_data.get("agg_party_contacts") or case_data.get("party_contacts")
        if contacts:
            try:
                items = json.loads(contacts) if isinstance(contacts, str) else contacts
                if isinstance(items, list):
                    for item in items:
                        if isinstance(item, dict):
                            name = self._text(item.get("party") or item.get("name"))
                            if name:
                                names.append(name)
            except Exception:
                pass

        # 从 client/opponent 名称补充
        for key in ("client_name", "opponent_name"):
            v = self._text(case_data.get(key))
            if v:
                names.append(v)

        # 当事人 + 证件号（如有）
        keys: list[str] = list(dict.fromkeys(n for n in names if n))
        for key in ("client_id_no", "opponent_id_no", "agg_client_id_no", "agg_opponent_id_no"):
            v = self._text(case_data.get(key))
            if v:
                keys.append(v)

        if not keys:
            self.report("conflict_skip", 22, "无当事人信息，跳过利冲检索")
            return False

        keys_str = ",".join(keys)

        # 排除自身案件（如果已经登记了 OA 案件号）
        exclude_ids: str = ""
        oa_case_no = self._text(case_data.get("agg_oa_lawcase_id")
                                 or case_data.get("oa_lawcase_id"))
        if oa_case_no:
            exclude_ids = oa_case_no

        self.report("conflict_search", 22, "正在进行利益冲突检索...")
        try:
            data = self._agent_get(
                "ConflictSearch",
                keys=keys_str,
                lawcaseIds=exclude_ids if exclude_ids else None,
                year=2,
                pageIndex=0,
                pageSize=20,
            )
        except RuntimeError as exc:
            self.report("conflict_error", 22, f"利冲检索接口调用失败: {exc}")
            return False  # 接口失败时不阻断立案，仅记日志

        if not isinstance(data, dict):
            self.report("conflict_skip", 22, "利冲检索返回格式异常，跳过")
            return False

        total = int(data.get("total") or 0)
        hits = data.get("data") or []

        if total == 0 or not hits:
            self.report("conflict_ok", 25, "利冲检索通过，无冲突案件")
            return False

        # 有冲突——整理详情返回，阻断立案
        briefs: list[dict[str, Any]] = []
        for row in hits[:10]:
            briefs.append({
                "lawcase_id": row.get("lawcaseId") or row.get("id"),
                "case_no": row.get("no") or row.get("preNo"),
                "wtr_names": row.get("wtrNames") or row.get("dsrNames"),
                "tos_names": row.get("tosNames"),
                "emp_names": row.get("empNames"),
                "cause": row.get("causeAction"),
                "status_name": row.get("statusName"),
                "charge_amount": row.get("chargeAmount"),
                "matched_keywords": row.get("matchedKeywords")
                                 or row.get("matched_keywords"),
            })

        # 组装可读的冲突详情，让律师能直接判断
        detail_lines: list[str] = [f"⚠️ 利益冲突检索命中 {total} 件："]
        for i, b in enumerate(briefs, 1):
            parts: list[str] = []
            if b.get("case_no"):
                parts.append(f"案号 {b['case_no']}")
            if b.get("wtr_names"):
                parts.append(f"委托人 {b['wtr_names']}")
            if b.get("tos_names"):
                parts.append(f"对方 {b['tos_names']}")
            if b.get("emp_names"):
                parts.append(f"律师 {b['emp_names']}")
            if b.get("cause"):
                parts.append(f"案由 {b['cause']}")
            if b.get("status_name"):
                parts.append(f"状态 {b['status_name']}")
            if b.get("matched_keywords"):
                parts.append(f"匹配 [{b['matched_keywords']}]")
            detail_lines.append(f"  {i}. {' | '.join(parts)}")
        if total > 10:
            detail_lines.append(f"  ……另有 {total - 10} 件未列出")
        detail_lines.append("请确认是否存在利益冲突，再决定是否继续提交。")
        msg = "\n".join(detail_lines)
        self.report("conflict_hit", 25, msg)
        return {
            "total": total,
            "hits": briefs,
            "keys": keys,
            "message": msg,
        }

    def _run_filing_via_agent_api(self, case_data: dict[str, Any]) -> OAResult:
        self.report("filing_start", 20, "开始通过 AgentAPI 登记案件...")

        # Step 1: 同案重复立案硬拦截。与利冲不同，重复立案不能用“忽略冲突”绕过。
        duplicate_result = self._duplicate_filing_precheck(case_data)
        if duplicate_result is not False:
            return OAResult(
                success=False,
                message=duplicate_result["message"],
                data={"duplicate_filing": duplicate_result},
            )

        # Step 2: 利益冲突检索（可跳过）
        if case_data.get("skip_conflict"):
            self.report("conflict_skip", 22, "用户选择跳过利冲检索")
            conflict_result = False
        else:
            conflict_result = self._conflict_search(case_data)
        if conflict_result is not False:
            return OAResult(
                success=False,
                message=conflict_result["message"],
                data={"conflict": conflict_result},
            )

        # Step 3: 构建立案载荷
        payload = self._build_case_registration_payload(case_data)
        self.report("filing_submit", 75, "正在提交案件登记...")
        result = self._agent_post("CaseRegistration", {"data": payload})
        self._repair_registered_instance_role_names(result, payload)
        self._repair_registered_proxy_permission(result, payload)
        self._verify_registered_case(result, payload)
        lawcase_id = self._extract_lawcase_id(result)
        if not lawcase_id:
            message = (
                "OA 未返回有效案件 ID，系统判定本次立案未真正写入。"
                "可能原因是 OA 拒绝重复立案或提交参数未通过校验。"
            )
            detail = self._agent_response_message(result)
            if detail:
                message += f" OA 返回: {detail}"
            return OAResult(
                success=False,
                message=message,
                data={"result": result, "lawcase_id": lawcase_id},
            )
        self.report("completed", 100, "OA 立案登记完成", {"result": result, "lawcase_id": lawcase_id})
        return OAResult(success=True, message="OA 立案登记完成", data={"result": result, "lawcase_id": lawcase_id})

    def _duplicate_filing_precheck(self, case_data: dict[str, Any]) -> bool | dict[str, Any]:
        if not self._agent_api_ready:
            return False

        matches = self._resolve_lawcase_matches(case_data)
        blockers = [
            row for row in matches
            if self._text(row.get("match_level")) in ("case_no", "exact")
        ]
        if not blockers:
            return False

        briefs = [self._duplicate_filing_brief(row) for row in blockers[:10]]
        lines = [f"OA 内已存在同案登记 {len(blockers)} 件，本次立案已拦截："]
        for index, row in enumerate(briefs, 1):
            parts = [
                part for part in (
                    f"案号 {row['case_no']}" if row.get("case_no") else "",
                    f"委托人 {row['wtr_names']}" if row.get("wtr_names") else "",
                    f"对方 {row['tos_names']}" if row.get("tos_names") else "",
                    f"案由 {row['cause']}" if row.get("cause") else "",
                    f"状态 {row['status_name']}" if row.get("status_name") else "",
                    f"匹配 {row['match_reasons']}" if row.get("match_reasons") else "",
                ) if part
            ]
            lines.append(f"  {index}. {' | '.join(parts)}")
        lines.append("请不要重复提交；如需补委托手续，请在已存在的 OA 案件上处理。")
        message = "\n".join(lines)
        self.report("duplicate_filing", 30, message, {"hits": briefs})
        return {
            "total": len(blockers),
            "hits": briefs,
            "message": message,
        }

    def _duplicate_filing_brief(self, row: dict[str, Any]) -> dict[str, Any]:
        reasons = row.get("match_reasons") or []
        if isinstance(reasons, list):
            reason_text = "、".join(str(item) for item in reasons if item)
        else:
            reason_text = self._text(reasons)
        return {
            "lawcase_id": row.get("id") or row.get("lawcaseId"),
            "case_no": row.get("no") or row.get("preNo"),
            "status": row.get("status"),
            "status_name": row.get("statusName"),
            "wtr_names": row.get("wtrNames") or row.get("dsrNames"),
            "tos_names": row.get("tosNames"),
            "emp_names": row.get("empNames"),
            "cause": row.get("causeAction"),
            "match_level": row.get("match_level"),
            "match_score": row.get("match_score"),
            "match_reasons": reason_text,
        }

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
            "ChargeMemo": self._text(case_data.get("charge_memo") or case_data.get("chargeMemo")),
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

    def _agent_response_message(self, result: Any) -> str:
        if result is None:
            return ""
        if isinstance(result, str):
            return result[:500]
        if not isinstance(result, dict):
            return self._text(result)[:500]

        candidates: list[Any] = []
        for key in (
            "message",
            "Message",
            "msg",
            "Msg",
            "error",
            "Error",
            "errorMessage",
            "ErrorMessage",
            "reason",
            "Reason",
            "status",
            "Status",
        ):
            candidates.append(result.get(key))

        nested = result.get("result") or result.get("data") or result.get("Data")
        if isinstance(nested, dict):
            for key in ("message", "Message", "msg", "error", "ErrorMessage"):
                candidates.append(nested.get(key))

        for value in candidates:
            text = self._text(value)
            if text and text.lower() not in {"ok", "success", "true"}:
                return text[:500]
        return ""

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

    # ─────────── 合伙人审批 ───────────

    def run_pending_approvals(self) -> OAResult:
        if not self._agent_api_ready:
            return OAResult(success=False, message="摩尚 OA 审批需要 AgentAPI Key 登录")

        self.report("pending_approvals", 35, "正在读取立案待审案件...")
        filing = self._get_lawcases_by_status(1)
        self.report("pending_approvals", 65, "正在读取结案待审案件...")
        closing = self._get_lawcases_by_status(4)
        data = {
            "filing": filing,
            "closing": closing,
            "counts": {"filing": len(filing), "closing": len(closing), "total": len(filing) + len(closing)},
            "fetched_at": self._now_iso(),
        }
        self.report("completed", 100, f"待审批 {data['counts']['total']} 件", data)
        return OAResult(success=True, message="待审批清单已刷新", data=data)

    def run_approval_check(self, lawcase_id: Any, approval_options: dict[str, Any]) -> OAResult:
        if not self._agent_api_ready:
            return OAResult(success=False, message="摩尚 OA 审批复核需要 AgentAPI Key 登录")
        case_id = self._require_lawcase_id(lawcase_id)
        self.report("approval_check", 20, "正在读取案件详情...")
        detail = self._get_case_detail(case_id)
        self.report("approval_check", 45, "正在进行资料、利冲和收费复核...")
        review = self._build_approval_review(case_id, detail, approval_options)
        self.report("completed", 100, "审批复核完成", review)
        return OAResult(success=True, message="审批复核完成", data=review)

    def run_approval_action(
        self,
        lawcase_id: Any,
        approved: bool,
        memo: str,
        confirm: bool,
        approval_options: dict[str, Any],
    ) -> OAResult:
        if not self._agent_api_ready:
            return OAResult(success=False, message="摩尚 OA 审批动作需要 AgentAPI Key 登录")
        case_id = self._require_lawcase_id(lawcase_id)
        memo = self._text(memo)
        if not approved and not memo:
            return OAResult(success=False, message="驳回必须填写审批意见")

        before = self._get_case_detail(case_id)
        old_status = int(before.get("status") or 0)
        if old_status != 1:
            return OAResult(
                success=False,
                message=f"案件不是立案待审状态，当前为 {before.get('statusName') or old_status}",
            )

        review = self._build_approval_review(case_id, before, approval_options) if approved else {
            "result": "not_required_for_rejection",
        }
        gate_errors = self._approval_gate_errors(approval_options, review) if approved else []
        expected_status = 3 if approved else 2
        preview = {
            "dry_run": not confirm,
            "action": "approve" if approved else "reject",
            "lawcase_id": case_id,
            "case_no": before.get("no") or before.get("preNo"),
            "old_status": old_status,
            "old_status_name": before.get("statusName"),
            "expected_status": expected_status,
            "expected_status_name": "办理中" if approved else "立案未通过",
            "memo": memo,
            "approval_review": review,
            "gate_passed": not gate_errors,
            "gate_errors": gate_errors,
        }
        if approved and gate_errors:
            return OAResult(success=False, message="审批通过被门禁阻止", data=preview)
        if not confirm:
            return OAResult(success=True, message="审批预演完成，未写入 OA", data=preview)

        self.report("approval_submit", 70, "正在提交审批意见...")
        self._post_lian_approval(case_id, approved, memo)
        after = None
        for _ in range(5):
            after = self._get_case_detail(case_id)
            if int(after.get("status") or 0) == expected_status:
                break
            time.sleep(1)
        new_status = int((after or {}).get("status") or 0)
        if new_status != expected_status:
            return OAResult(
                success=False,
                message=f"审批请求已返回，但回读状态失败：期望 {expected_status}，实际 {new_status}",
                data={**preview, "new_status": new_status, "verified": False},
            )
        data = {**preview, "dry_run": False, "verified": True, "new_status": new_status, "new_status_name": after.get("statusName")}
        self.report("completed", 100, "审批已提交并回读验证", data)
        return OAResult(success=True, message="审批已提交并回读验证", data=data)

    def run_approval_monitor_snapshot(
        self,
        approval_options: dict[str, Any],
        monitor_state_path: str | None,
    ) -> OAResult:
        if not self._agent_api_ready:
            return OAResult(success=False, message="摩尚 OA 审批提醒需要 AgentAPI Key 登录")
        path = Path(monitor_state_path) if monitor_state_path else self.cookies_dir / "approval-monitor.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        previous = self._read_monitor_state(path)
        pending = self._get_lawcases_by_status(1)
        closing = self._get_lawcases_by_status(4)
        current_ids = {self._row_id(row) for row in pending + closing if self._row_id(row)}
        reminded = set(str(x) for x in previous.get("reminded_ids", []))
        new_rows = [row for row in pending if self._row_id(row) and self._row_id(row) not in reminded]
        next_state = {
            "updated_at": self._now_iso(),
            "reminded_ids": sorted((reminded | {self._row_id(row) for row in new_rows if self._row_id(row)}) & current_ids),
        }
        path.write_text(json.dumps(next_state, ensure_ascii=False, indent=2))
        data = {
            "filing": pending,
            "closing": closing,
            "new_filing": new_rows,
            "counts": {"filing": len(pending), "closing": len(closing), "new_filing": len(new_rows)},
            "state_path": str(path),
            "fetched_at": self._now_iso(),
        }
        return OAResult(success=True, message="审批提醒快照已刷新", data=data)

    def run_download_engagement_documents(self, lawcase_id: Any, output_dir: str | None) -> OAResult:
        if not self._agent_api_ready or not self._http:
            return OAResult(success=False, message="摩尚 OA 文书下载需要 AgentAPI Key 登录")
        case_id = self._require_lawcase_id(lawcase_id)
        out_dir = Path(output_dir or Path.home() / "Downloads").expanduser()
        out_dir.mkdir(parents=True, exist_ok=True)

        self.report("document_prepare", 25, "正在读取 OA 案件详情...")
        detail = self._get_case_detail(case_id)
        templates = self._agent_get("GetWordTemplates", lawcaseId=case_id) or []
        template_names = self._template_names(templates)
        requested = self._engagement_template_names(detail, template_names)
        if not requested:
            return OAResult(success=False, message="OA 当前案件未返回可用的委托手续模板")

        self.report("document_export", 55, "正在生成并下载委托手续...")
        export = self._agent_get("ExportWordTemplates", lawcaseId=case_id, fileTemplateNames=",".join(requested))
        if not isinstance(export, dict) or not export.get("downloadUrl"):
            return OAResult(success=False, message=f"OA 未返回文书下载地址: {export!r}")
        download_url = str(export["downloadUrl"])
        if download_url.startswith("/"):
            download_url = urljoin(self.site_url + "/", download_url.lstrip("/"))
        resp = self._http.get(download_url)
        resp.raise_for_status()
        filename = self._download_filename(resp.headers.get("content-disposition"), detail, requested)
        target = self._unique_output_path(out_dir, filename)
        target.write_bytes(resp.content)
        if target.stat().st_size <= 0:
            raise RuntimeError("下载的委托手续文件为空")
        if target.suffix.lower() == ".docx" and not zipfile.is_zipfile(target):
            raise RuntimeError(f"下载的委托手续不是有效 docx: {target.name}")

        data = {
            "lawcase_id": case_id,
            "case_no": detail.get("no") or detail.get("preNo"),
            "templates": requested,
            "path": str(target),
            "filename": target.name,
            "size_bytes": target.stat().st_size,
            "client_is_legal_person": self._has_legal_person_principal(detail),
        }
        self.report("completed", 100, "委托手续已下载", data)
        return OAResult(success=True, message="委托手续已下载", data=data)

    def run_resolve_engagement_lawcase(self, case_data: dict[str, Any]) -> OAResult:
        if not self._agent_api_ready or not self._http:
            return OAResult(success=False, message="摩尚 OA 案件查找需要 AgentAPI Key 登录")
        self.report("document_resolve", 25, "正在 OA 系统查找对应案件...")
        matches = self._resolve_lawcase_matches(case_data)
        if not matches:
            return OAResult(success=False, message="OA 系统未找到与本案匹配的案件")
        row = self._pick_engagement_lawcase_match(matches)
        if row is None:
            brief = [
                {
                    "lawcase_id": row.get("id") or row.get("lawcaseId"),
                    "case_no": row.get("no") or row.get("preNo"),
                    "wtr_names": row.get("wtrNames") or row.get("dsrNames"),
                    "tos_names": row.get("tosNames"),
                    "emp_names": row.get("empNames"),
                    "cause": row.get("causeAction"),
                    "status_name": row.get("statusName"),
                    "match_score": row.get("match_score"),
                    "match_reasons": row.get("match_reasons"),
                    "match_level": row.get("match_level"),
                }
                for row in matches[:8]
            ]
            return OAResult(
                success=False,
                message="OA 系统找到多个疑似案件，无法自动确定下载哪一个",
                data={"matches": brief},
            )
        lawcase_id = int(row.get("id") or row.get("lawcaseId"))
        data = {
            "lawcase_id": lawcase_id,
            "case_no": row.get("no") or row.get("preNo"),
            "wtr_names": row.get("wtrNames") or row.get("dsrNames"),
            "tos_names": row.get("tosNames"),
            "emp_names": row.get("empNames"),
            "cause": row.get("causeAction"),
            "status_name": row.get("statusName"),
            "match_score": row.get("match_score"),
            "match_reasons": row.get("match_reasons"),
            "match_level": row.get("match_level"),
        }
        self.report("completed", 100, "已找到 OA 对应案件", data)
        return OAResult(success=True, message="已找到 OA 对应案件", data=data)

    def _get_lawcases_by_status(self, status: int, page_size: int = 100) -> list[dict[str, Any]]:
        rows: list[dict[str, Any]] = []
        page_index = 0
        total = 0
        while True:
            payload = self._agent_get("GetLawcases", status=status, pageIndex=page_index, pageSize=page_size)
            if not isinstance(payload, dict):
                return rows
            page = payload.get("data") or []
            if not isinstance(page, list) or not page:
                return rows
            rows.extend(row for row in page if isinstance(row, dict))
            total = int(payload.get("total") or total or len(rows))
            if len(rows) >= total or len(page) < page_size:
                return rows
            page_index += 1

    def _template_names(self, templates: Any) -> list[str]:
        if not isinstance(templates, list):
            return []
        names: list[str] = []
        for item in templates:
            if isinstance(item, str):
                name = self._text(item)
            elif isinstance(item, dict):
                name = self._text(item.get("name") or item.get("templateName") or item.get("fileTemplateName"))
            else:
                name = ""
            if name and name not in names:
                names.append(name)
        return names

    def _engagement_template_names(self, detail: dict[str, Any], available: list[str]) -> list[str]:
        base_text = " ".join(
            self._text(detail.get(key))
            for key in ("baseTypeName", "baseType", "caseCategoryName", "caseCategory", "causeAction")
        )
        preferred = "仲裁委托手续" if "仲裁" in base_text else "民事委托手续"
        names: list[str] = []
        if preferred in available:
            names.append(preferred)
        else:
            for candidate in available:
                if "委托手续" in candidate:
                    names.append(candidate)
                    break
        if self._has_legal_person_principal(detail):
            companion = "法人代表证明书（仲裁）" if "仲裁" in base_text else "法定代表身份证明书"
            fallback = "负责人证明书"
            if companion in available:
                names.append(companion)
            elif fallback in available:
                names.append(fallback)
        return [name for idx, name in enumerate(names) if name and name not in names[:idx]]

    def _has_legal_person_principal(self, detail: dict[str, Any]) -> bool:
        clients = [row for row in (detail.get("clients") or []) if isinstance(row, dict)]
        principals = [row for row in clients if row.get("roleType") == 0]
        legal_words = ("公司", "集团", "有限", "股份", "法人", "企业", "合作社", "事务所")
        for row in principals:
            text = " ".join(self._text(row.get(key)) for key in ("name", "identityTypeName", "clientTypeName", "typeName"))
            if any(word in text for word in legal_words):
                return True
            identity = str(row.get("identityType") or row.get("IdentityTypeId") or "")
            if identity and identity not in {"", "1", "自然人"} and "自然" not in text:
                return True
        if not principals:
            names = self._split_names(detail.get("wtrNames") or detail.get("dsrNames"))
            return any(any(word in name for word in legal_words) for name in names)
        return False

    def _download_filename(self, content_disposition: str | None, detail: dict[str, Any], templates: list[str]) -> str:
        if content_disposition:
            match = re.search(r"filename\*=UTF-8''([^;]+)", content_disposition, re.IGNORECASE)
            if match:
                name = unquote(match.group(1)).strip()
                if name:
                    return self._safe_filename(name)
            match = re.search(r'filename="?([^";]+)"?', content_disposition, re.IGNORECASE)
            if match:
                name = match.group(1).strip()
                if name:
                    return self._safe_filename(name)
        case_no = self._text(detail.get("no") or detail.get("preNo") or f"OA{detail.get('id') or '案件'}")
        label = "+".join(templates) if templates else "委托手续"
        return self._safe_filename(f"{case_no}-{label}.docx")

    def _safe_filename(self, value: str) -> str:
        name = re.sub(r'[<>:"/\\|?*\r\n]+', "_", value).strip(" ._")
        return name or "OA委托手续.docx"

    def _unique_output_path(self, out_dir: Path, filename: str) -> Path:
        path = out_dir / filename
        if not path.exists():
            return path
        stem = path.stem
        suffix = path.suffix or ".docx"
        for index in range(2, 1000):
            candidate = out_dir / f"{stem}_{index}{suffix}"
            if not candidate.exists():
                return candidate
        return out_dir / f"{stem}_{int(time.time())}{suffix}"

    def _pick_engagement_lawcase_match(self, matches: list[dict[str, Any]]) -> dict[str, Any] | None:
        if not matches:
            return None
        top = matches[0]
        top_score = int(top.get("match_score") or 0)
        top_level = self._text(top.get("match_level"))
        if top_level == "case_no":
            return top
        if len(matches) == 1:
            return top
        second_score = int(matches[1].get("match_score") or 0)
        reasons = set(top.get("match_reasons") or [])
        has_core_parties = "委托人匹配" in reasons and "对方匹配" in reasons
        has_strong_identity = has_core_parties and ("案由匹配" in reasons or "经办律师匹配" in reasons)
        if top_level == "exact" and top_score >= 95 and top_score - second_score >= 10:
            return top
        if top_level == "exact" and top_score >= 95 and second_score < 95:
            return top
        if has_strong_identity and top_score - second_score >= 10:
            return top
        return None

    def _resolve_lawcase_matches(self, case_data: dict[str, Any]) -> list[dict[str, Any]]:
        case_no = self._text(case_data.get("agg_case_no") or case_data.get("case_no"))
        cause = self._normalize_name(self._text(case_data.get("agg_cause") or case_data.get("cause")))
        principals, opponents = self._engagement_principal_opponent_names(case_data)
        third_parties = self._party_names(case_data, "third_parties", "agg_third_parties")
        lawyers = self._lawyer_names(case_data)
        cause_text = self._text(case_data.get("agg_cause") or case_data.get("cause"))
        keywords = [case_no, *principals, *opponents, *third_parties, cause_text, self._text(case_data.get("name"))]
        rows: dict[int, dict[str, Any]] = {}
        for keyword in dict.fromkeys(k for k in keywords if self._text(k)):
            for row in self._get_case_list_all(keyword):
                row_id = int(row.get("id") or row.get("lawcaseId") or 0)
                if row_id:
                    rows[row_id] = row

        principal_keys = {self._normalize_name(name) for name in principals if self._normalize_name(name)}
        opponent_keys = {self._normalize_name(name) for name in opponents if self._normalize_name(name)}
        lawyer_keys = {self._normalize_name(name) for name in lawyers if self._normalize_name(name)}
        scored: list[tuple[int, dict[str, Any]]] = []
        for row in rows.values():
            row_case_no = self._normalize_name(self._text(row.get("no") or row.get("preNo")))
            row_principals = {self._normalize_name(x) for x in self._split_names(row.get("wtrNames") or row.get("dsrNames"))}
            row_opponents = {self._normalize_name(x) for x in self._split_names(row.get("tosNames"))}
            row_lawyers = {self._normalize_name(x) for x in self._split_names(row.get("empNames"))}
            row_cause = self._normalize_name(self._text(row.get("causeAction")))
            score = 0
            reasons: list[str] = []
            if case_no and self._normalize_name(case_no) == row_case_no:
                score += 150
                reasons.append("案号匹配")
            if principal_keys and principal_keys & row_principals:
                score += 40
                reasons.append("委托人匹配")
            if opponent_keys and opponent_keys & row_opponents:
                score += 40
                reasons.append("对方匹配")
            if cause and row_cause and cause == row_cause:
                score += 25
                reasons.append("案由匹配")
            if lawyer_keys and lawyer_keys & row_lawyers:
                score += 15
                reasons.append("经办律师匹配")
            if score <= 0:
                continue
            row = dict(row)
            row["match_score"] = score
            row["match_reasons"] = reasons
            if "案号匹配" in reasons:
                row["match_level"] = "case_no"
            elif "委托人匹配" in reasons and "对方匹配" in reasons and ("案由匹配" in reasons or "经办律师匹配" in reasons):
                row["match_level"] = "exact"
            elif "委托人匹配" in reasons and "对方匹配" in reasons:
                row["match_level"] = "strong_candidate"
            else:
                row["match_level"] = "candidate"
            scored.append((score, row))
        scored.sort(key=lambda item: item[0], reverse=True)
        return [row for _, row in scored]

    def _engagement_principal_opponent_names(self, case_data: dict[str, Any]) -> tuple[list[str], list[str]]:
        plaintiffs = self._party_names(case_data, "plaintiffs", "agg_plaintiffs")
        defendants = self._party_names(case_data, "defendants", "agg_defendants")
        third_parties = self._party_names(case_data, "third_parties", "agg_third_parties")
        proxy_side = self._text(
            case_data.get("proxy_side")
            or case_data.get("current_instance_role")
            or case_data.get("instance_role")
            or case_data.get("agg_our_side")
            or case_data.get("our_side")
        )
        if "被告" in proxy_side or "被申请" in proxy_side:
            return defendants, [*plaintiffs, *third_parties]
        if "第三" in proxy_side:
            return third_parties, [*plaintiffs, *defendants]
        if "原告" in proxy_side or "申请" in proxy_side:
            return plaintiffs, [*defendants, *third_parties]
        contacts = self._party_contact_rows(case_data)
        principals = [self._text(row.get("name") or row.get("party")) for row in contacts if row.get("is_our_side") is True]
        opponents = [
            self._text(row.get("name") or row.get("party"))
            for row in contacts
            if row.get("is_our_side") is False
        ]
        principals = [name for name in principals if name]
        opponents = [name for name in opponents if name]
        return (principals or plaintiffs, opponents or defendants)

    def _party_contact_rows(self, case_data: dict[str, Any]) -> list[dict[str, Any]]:
        raw = case_data.get("agg_party_contacts") or case_data.get("party_contacts")
        if not raw:
            return []
        try:
            items = json.loads(raw) if isinstance(raw, str) else raw
        except Exception:
            return []
        return [item for item in items if isinstance(item, dict)] if isinstance(items, list) else []

    def _get_case_detail(self, lawcase_id: int) -> dict[str, Any]:
        result = self._agent_get("GetLawcaseDetail", lawcaseId=lawcase_id)
        if not isinstance(result, dict):
            raise RuntimeError(f"OA 案件详情返回格式异常: {result!r}")
        return result

    def _build_approval_review(
        self,
        lawcase_id: int,
        detail: dict[str, Any],
        approval_options: dict[str, Any],
    ) -> dict[str, Any]:
        entity = self._get_case_entity_for_approval(lawcase_id, detail) or {}
        completeness = self._completeness_review(detail, entity)
        conflict = self._conflict_review(detail, lawcase_id)
        duplicate = self._duplicate_filing_review(detail, lawcase_id)
        local_case = self._local_case_check(detail, approval_options)
        risk_charge = self._risk_charge_review(detail, entity, approval_options.get("risk_fee_amount"))
        fee_review = self._fee_reasonableness_review(detail, entity, approval_options)
        recommendation = self._approval_recommendation(completeness, conflict, duplicate, local_case, risk_charge, fee_review)
        return {
            "lawcase_id": lawcase_id,
            "case_no": detail.get("no") or detail.get("preNo"),
            "status": detail.get("status"),
            "status_name": detail.get("statusName"),
            "summary": self._approval_case_summary(detail, entity),
            "completeness_review": completeness,
            "conflict_review": conflict,
            "duplicate_filing_review": duplicate,
            "local_case_check": local_case,
            "risk_charge_review": risk_charge,
            "fee_reasonableness_review": fee_review,
            "fallback_review": {
                "result": "manual_rejection_available",
                "templates": [
                    "资料不完整",
                    "利冲待复核",
                    "风险代理材料不足",
                    "收费过低需调整",
                    "收费过高需说明",
                    "收费方式不匹配",
                    "其他",
                ],
            },
            "recommendation": recommendation,
        }

    def _approval_case_summary(self, detail: dict[str, Any], entity: dict[str, Any]) -> dict[str, Any]:
        charge_method = detail.get("chargeMethodName") or entity.get("chargeMethd")
        is_risk_charge = self._is_risk_charge(detail, entity)
        return {
            "wtr_names": detail.get("wtrNames") or detail.get("dsrNames"),
            "tos_names": detail.get("tosNames"),
            "emp_names": detail.get("empNames"),
            "cause": detail.get("causeAction") or detail.get("caseHeadName"),
            "base_type": detail.get("baseTypeName") or detail.get("baseType"),
            "case_category": detail.get("caseCategoryName") or detail.get("caseCategory"),
            "charge_method": charge_method,
            "is_risk_charge": is_risk_charge,
            "risk_charge_label": "风险收费" if is_risk_charge else "非风险收费",
            "charge_amount": self._float_or_none(detail.get("chargeAmount")),
            "subject_amount": self._float_or_none(entity.get("Biaodi") or detail.get("biaodi")),
            "received": self._float_or_none(detail.get("yishou")),
            "unreceived": self._float_or_none(detail.get("weishou")),
            "shouli_date": detail.get("shouliDate"),
            "case_summary": self._first_text(
                detail,
                entity,
                "caseSummary",
                "CaseSummary",
                "summary",
                "Summary",
                "案情摘要",
            ),
            "case_memo": self._first_text(
                detail,
                entity,
                "caseMemo",
                "CaseMemo",
                "memo",
                "Memo",
                "note",
                "Note",
                "情况说明",
                "备注",
            ),
            "charge_memo": self._first_text(
                detail,
                entity,
                "chargeMemo",
                "ChargeMemo",
                "charge_memo",
                "收费说明",
                "风险收费说明",
            ),
            "proxy_permission": self._first_text(
                detail,
                entity,
                "WTQXContent",
                "wtqxContent",
                "proxyPermission",
                "proxy_permission",
                "代理权限",
                "代理事项",
            ),
        }

    def _first_text(self, *sources: Any) -> str:
        keys = [item for item in sources if isinstance(item, str)]
        containers = [item for item in sources if isinstance(item, dict)]
        for container in containers:
            for key in keys:
                text = self._text(container.get(key))
                if text:
                    return text
        return ""

    def _completeness_review(self, detail: dict[str, Any], entity: dict[str, Any]) -> dict[str, Any]:
        principals, opponents = self._approval_party_names(detail)
        missing = []
        checks = {
            "委托人": principals,
            "对方": opponents,
            "经办律师": detail.get("empNames") or detail.get("employees"),
            "案由": detail.get("causeAction") or detail.get("caseHeadName"),
            "案件阶段": detail.get("instances"),
            "收费方式": detail.get("chargeMethodName") or entity.get("chargeMethd"),
            "委托收费金额": detail.get("chargeAmount"),
            "案情摘要": detail.get("caseSummary"),
        }
        for label, value in checks.items():
            if value in (None, "", []):
                missing.append(label)
        return {"result": "blocked" if missing else "complete", "missing": missing}

    def _conflict_review(self, detail: dict[str, Any], lawcase_id: int) -> dict[str, Any]:
        principals, opponents = self._approval_party_names(detail)
        principal_keys = {self._normalize_name(name) for name in principals}
        opponent_keys = {self._normalize_name(name) for name in opponents}
        blockers: list[str] = []
        findings: list[dict[str, Any]] = []
        if principal_keys & opponent_keys:
            blockers.append("本案委托人与对方存在同名主体，必须更正或核实主体身份")

        searched: dict[str, list[dict[str, Any]]] = {}
        for name in dict.fromkeys(principals + opponents):
            searched[name] = self._get_case_list_all(name)

        seen = set()
        for searched_name, rows in searched.items():
            key = self._normalize_name(searched_name)
            for row in rows:
                row_id = int(row.get("id") or row.get("lawcaseId") or 0)
                if row_id == lawcase_id:
                    continue
                row_principals = {self._normalize_name(x) for x in self._split_names(row.get("wtrNames") or row.get("dsrNames"))}
                row_opponents = {self._normalize_name(x) for x in self._split_names(row.get("tosNames"))}
                relation = None
                if key in principal_keys and key in row_opponents:
                    relation = "本案委托人曾/正在作为本所案件对方"
                elif key in opponent_keys and key in row_principals:
                    relation = "本案对方曾/正在作为本所委托人"
                if not relation:
                    continue
                identity = (row_id, relation, key)
                if identity in seen:
                    continue
                seen.add(identity)
                status = int(row.get("status") or 0)
                findings.append(
                    {
                        "matched_name": searched_name,
                        "relation": relation,
                        "severity": "high" if status in ACTIVE_CASE_STATUSES else "review",
                        "case_id": row_id,
                        "case_no": row.get("no") or row.get("preNo"),
                        "status": status,
                        "status_name": row.get("statusName"),
                        "wtr_names": row.get("wtrNames"),
                        "tos_names": row.get("tosNames"),
                        "cause": row.get("causeAction"),
                    }
                )

        return {
            "result": "blocked" if blockers else ("manual_review_required" if findings else "no_exact_adverse_match"),
            "principals": principals,
            "opponents": opponents,
            "blockers": blockers,
            "findings": findings,
            "limitations": [
                "仅检索 OA 中可见案件并按规范化后的主体名称精确比对",
                "同名、曾用名、关联企业、实际控制人和未录入 OA 事项仍须人工核验",
                "命中是风险线索，不等于已经构成法律上的利益冲突",
            ],
        }

    def _duplicate_filing_review(self, detail: dict[str, Any], lawcase_id: int) -> dict[str, Any]:
        principals, opponents = self._approval_party_names(detail)
        principal_keys = {self._normalize_name(name) for name in principals if self._normalize_name(name)}
        opponent_keys = {self._normalize_name(name) for name in opponents if self._normalize_name(name)}
        cause = self._normalize_name(self._text(detail.get("causeAction") or detail.get("caseHeadName")))
        findings: list[dict[str, Any]] = []
        warnings: list[dict[str, Any]] = []
        blockers: list[str] = []

        searched: dict[str, list[dict[str, Any]]] = {}
        for name in dict.fromkeys(principals + opponents):
            searched[name] = self._get_case_list_all(name)

        seen: set[int] = set()
        for rows in searched.values():
            for row in rows:
                row_id = int(row.get("id") or row.get("lawcaseId") or 0)
                if not row_id or row_id == lawcase_id or row_id in seen:
                    continue
                status = int(row.get("status") or 0)
                if status not in ACTIVE_CASE_STATUSES:
                    continue
                row_principals = {self._normalize_name(x) for x in self._split_names(row.get("wtrNames") or row.get("dsrNames"))}
                row_opponents = {self._normalize_name(x) for x in self._split_names(row.get("tosNames"))}
                principal_hit = sorted(principal_keys & row_principals)
                opponent_hit = sorted(opponent_keys & row_opponents)
                if not principal_hit and not opponent_hit:
                    continue
                row_cause = self._normalize_name(self._text(row.get("causeAction")))
                cause_hit = bool(cause and row_cause and cause == row_cause)
                item = {
                    "case_id": row_id,
                    "case_no": row.get("no") or row.get("preNo"),
                    "status": status,
                    "status_name": row.get("statusName"),
                    "wtr_names": row.get("wtrNames"),
                    "tos_names": row.get("tosNames"),
                    "emp_names": row.get("empNames"),
                    "cause": row.get("causeAction"),
                    "matched_principals": principal_hit,
                    "matched_opponents": opponent_hit,
                    "cause_matched": cause_hit,
                }
                seen.add(row_id)
                if principal_hit and opponent_hit and cause_hit:
                    findings.append(item)
                    blockers.append(
                        f"OA 内已有相同委托人、相同对方、相同案由的在办案件: {item.get('case_no') or row_id}"
                    )
                else:
                    warnings.append(item)

        return {
            "result": "blocked" if findings else ("advisory_matches" if warnings else "clear"),
            "principals": principals,
            "opponents": opponents,
            "cause": detail.get("causeAction") or detail.get("caseHeadName"),
            "blockers": blockers,
            "findings": findings,
            "warnings": warnings[:20],
            "limitations": ["重复立案检测按委托人、对方、案由精确匹配；系列案件或同名主体仍需人工复核"],
        }

    def _local_case_check(self, detail: dict[str, Any], approval_options: dict[str, Any]) -> dict[str, Any]:
        principals, opponents = self._approval_party_names(detail)
        names = [name for name in dict.fromkeys(principals + opponents) if self._text(name)]
        if not names:
            return {"result": "not_applicable", "findings": [], "warnings": ["待审案件没有可检索的当事人名称"]}
        db_path = self._text(approval_options.get("local_case_db")) or str(
            Path.home() / "Library" / "Application Support" / "CaseBoard" / "caseboard.db"
        )
        path = Path(db_path).expanduser()
        if not path.exists():
            return {"result": "unavailable", "db_path": str(path), "findings": [], "warnings": ["未找到案件看板本地数据库，已跳过立重检查"]}

        name_keys = {self._normalize_name(name): name for name in names}
        findings: list[dict[str, Any]] = []
        try:
            conn = sqlite3.connect(str(path))
            conn.row_factory = sqlite3.Row
            rows = conn.execute(
                "SELECT id, name, case_no, cause, agg_cause, agg_plaintiffs, agg_defendants, agg_third_parties, workflow_status, case_status "
                "FROM cases ORDER BY updated_at DESC LIMIT 1000"
            ).fetchall()
        except Exception as exc:
            return {"result": "unavailable", "db_path": str(path), "findings": [], "warnings": [f"本地立重检查失败: {exc}"]}
        finally:
            try:
                conn.close()
            except Exception:
                pass

        for row in rows:
            parties = [
                ("原告/申请人", self._json_names(row["agg_plaintiffs"])),
                ("被告/被申请人", self._json_names(row["agg_defendants"])),
                ("第三人", self._json_names(row["agg_third_parties"])),
            ]
            for role, party_names in parties:
                for party_name in party_names:
                    key = self._normalize_name(party_name)
                    if key not in name_keys:
                        continue
                    findings.append(
                        {
                            "matched_name": name_keys[key],
                            "local_case_id": row["id"],
                            "case_name": row["name"],
                            "case_no": row["case_no"],
                            "cause": row["agg_cause"] or row["cause"],
                            "role": role,
                            "workflow_status": row["workflow_status"],
                            "case_status": row["case_status"],
                        }
                    )
        return {
            "result": "advisory_matches" if findings else "clear",
            "db_path": str(path),
            "findings": findings[:30],
            "limitations": ["本地立重检查只做当事人名称精确匹配，命中不等于必须拒绝审批"],
        }

    def _risk_charge_review(
        self,
        detail: dict[str, Any],
        entity: dict[str, Any],
        risk_fee_amount: Any = None,
    ) -> dict[str, Any]:
        if not self._is_risk_charge(detail, entity):
            return {"result": "not_applicable", "charge_method": detail.get("chargeMethodName")}

        description = " ".join(
            str(value or "")
            for value in (
                detail.get("baseTypeName"),
                detail.get("caseCategoryName"),
                detail.get("causeAction"),
                detail.get("caseHeadName"),
                detail.get("caseSummary"),
            )
        )
        prohibited_hits = [keyword for keyword in RISK_PROHIBITED_KEYWORDS if keyword in description]
        base_type = str(detail.get("baseTypeName") or detail.get("baseType") or "")
        if "行政" in base_type and "行政诉讼" not in prohibited_hits:
            prohibited_hits.append("行政诉讼")
        subject_amount = self._decimal_value(entity.get("Biaodi") or detail.get("biaodi"))
        agreed_fee = self._decimal_value(risk_fee_amount if risk_fee_amount is not None else detail.get("chargeAmount"))
        cap = self._risk_fee_cap(subject_amount)
        blockers: list[str] = []
        warnings: list[str] = []
        if prohibited_hits:
            blockers.append("案件类型属于风险代理禁止适用范围: " + "、".join(prohibited_hits))
        if subject_amount is None:
            blockers.append("缺少用于计算风险代理上限的标的额")
        if agreed_fee is None:
            blockers.append("缺少风险代理各环节服务费合计最高金额")
        if cap is not None and agreed_fee is not None and agreed_fee > cap:
            blockers.append(f"约定风险代理费 {agreed_fee} 元超过分段上限 {cap} 元")
        if risk_fee_amount is None:
            warnings.append("暂将 OA 的委托收费金额视为风险代理各环节收费合计最高额；如含义不同请在复核中调整")
        return {
            "result": "blocked" if blockers else "documents_confirmation_required",
            "charge_method": detail.get("chargeMethodName"),
            "subject_amount": self._decimal_to_float(subject_amount),
            "agreed_max_fee": self._decimal_to_float(agreed_fee),
            "graduated_cap": self._decimal_to_float(cap),
            "prohibited_matches": prohibited_hits,
            "blockers": blockers,
            "warnings": warnings,
            "required_documents": [
                "专门的书面风险代理合同",
                "醒目标明风险代理含义、禁止范围和最高收费限制",
                "明确目标、双方风险责任且不限制上诉、撤诉、调解、和解权利",
            ],
            "basis": "司发通〔2021〕87号第四至七项",
        }

    def _fee_reasonableness_review(
        self,
        detail: dict[str, Any],
        entity: dict[str, Any],
        approval_options: dict[str, Any],
    ) -> dict[str, Any]:
        thresholds = {
            "min_fee": self._decimal_value(approval_options.get("min_fee")) or Decimal("3000"),
            "low_ratio": self._decimal_value(approval_options.get("low_ratio")) or Decimal("0.005"),
            "high_ratio": self._decimal_value(approval_options.get("high_ratio")) or Decimal("0.30"),
            "risk_base_fee_min": self._decimal_value(approval_options.get("risk_base_fee_min")) or Decimal("0"),
        }
        charge_method = self._text(detail.get("chargeMethodName"))
        charge_amount = self._decimal_value(detail.get("chargeAmount"))
        subject_amount = self._decimal_value(entity.get("Biaodi") or detail.get("biaodi"))
        received = self._decimal_value(detail.get("yishou"))
        unreceived = self._decimal_value(detail.get("weishou"))
        issues: list[dict[str, Any]] = []
        blockers: list[str] = []
        warnings: list[str] = []

        if charge_amount is None:
            issues.append({"severity": "补正", "message": "委托收费金额缺失"})
        elif charge_amount <= 0:
            issues.append({"severity": "补正", "message": "委托收费为 0 或负数"})
        elif charge_amount < thresholds["min_fee"]:
            issues.append({"severity": "复核", "message": f"委托收费低于最低提醒线 {thresholds['min_fee']} 元"})

        if subject_amount and subject_amount > 0 and charge_amount is not None:
            ratio = charge_amount / subject_amount
            if ratio < thresholds["low_ratio"]:
                issues.append({"severity": "复核", "message": f"收费占标的额比例 {ratio:.2%}，低于低收费提醒线"})
            if ratio > thresholds["high_ratio"]:
                issues.append({"severity": "复核", "message": f"收费占标的额比例 {ratio:.2%}，高于高收费提醒线"})

        if self._is_risk_charge(detail, entity) and (charge_amount is None or charge_amount <= thresholds["risk_base_fee_min"]):
            warnings.append("风险代理案件需确认是否另有基础收费或风险代理书面说明")
        if "另案已收" in charge_method and not self._text(detail.get("caseMemo") or detail.get("caseSummary")):
            issues.append({"severity": "补正", "message": "收费方式为另案已收，但缺少对应说明"})
        if received is not None and unreceived is not None and charge_amount is not None and received + unreceived > charge_amount * Decimal("1.05"):
            issues.append({"severity": "复核", "message": "已收与未收合计明显超过委托收费金额"})

        result = "reasonable"
        if any(item["severity"] == "补正" for item in issues):
            result = "correction_required"
        elif issues or warnings:
            result = "manual_review_required"
        return {
            "result": result,
            "charge_method": charge_method,
            "charge_amount": self._decimal_to_float(charge_amount),
            "subject_amount": self._decimal_to_float(subject_amount),
            "received": self._decimal_to_float(received),
            "unreceived": self._decimal_to_float(unreceived),
            "thresholds": {
                "min_fee": self._decimal_to_float(thresholds["min_fee"]),
                "low_ratio": self._decimal_to_float(thresholds["low_ratio"]),
                "high_ratio": self._decimal_to_float(thresholds["high_ratio"]),
                "risk_base_fee_min": self._decimal_to_float(thresholds["risk_base_fee_min"]),
            },
            "issues": issues,
            "warnings": warnings,
            "blockers": blockers,
            "limitations": ["收费合理性是管理提醒，不替代合伙人结合案情、客户关系和律所政策作最终判断"],
        }

    def _approval_recommendation(
        self,
        completeness: dict[str, Any],
        conflict: dict[str, Any],
        duplicate: dict[str, Any],
        local_case: dict[str, Any],
        risk_charge: dict[str, Any],
        fee_review: dict[str, Any],
    ) -> dict[str, Any]:
        hard_reasons: list[str] = []
        correction_reasons: list[str] = []
        manual_reasons: list[str] = []

        if completeness.get("result") == "blocked":
            correction_reasons.extend(f"资料缺失：{x}" for x in completeness.get("missing") or [])
        hard_reasons.extend(conflict.get("blockers") or [])
        if conflict.get("findings"):
            manual_reasons.append("利冲检索存在同名命中，需要合伙人复核")
        hard_reasons.extend(duplicate.get("blockers") or [])
        if duplicate.get("warnings"):
            manual_reasons.append("OA 内存在当事人部分重叠案件，需要核对是否重复或关联立案")
        if local_case.get("findings"):
            manual_reasons.append("案件看板本地系统存在当事人立重命中，需要合伙人查看")
        hard_reasons.extend(risk_charge.get("blockers") or [])
        if risk_charge.get("result") == "documents_confirmation_required":
            manual_reasons.append("风险代理需确认书面合同、醒目告知和风险提示")
        if fee_review.get("result") == "correction_required":
            correction_reasons.extend(item.get("message") for item in fee_review.get("issues") or [] if item.get("message"))
        elif fee_review.get("result") == "manual_review_required":
            manual_reasons.append("收费存在过低、过高或方式不匹配提醒")

        if hard_reasons:
            result = "block_approval"
            label = "不得通过"
        elif correction_reasons:
            result = "recommend_reject_for_correction"
            label = "建议驳回补正"
        elif manual_reasons:
            result = "manual_review_required"
            label = "需人工判断"
        else:
            result = "recommend_approve"
            label = "建议通过"
        return {
            "result": result,
            "label": label,
            "reasons": hard_reasons + correction_reasons + manual_reasons,
        }

    def _approval_gate_errors(self, approval_options: dict[str, Any], review: dict[str, Any]) -> list[str]:
        errors = [f"资料不完整: {value}" for value in review.get("completeness_review", {}).get("missing") or []]
        conflict = review.get("conflict_review", {})
        errors.extend(conflict.get("blockers") or [])
        if conflict.get("findings") and not (approval_options.get("conflict_reviewed") and self._text(approval_options.get("conflict_memo"))):
            errors.append("存在利冲检索命中，须填写合伙人复核结论")
        duplicate = review.get("duplicate_filing_review", {})
        errors.extend(duplicate.get("blockers") or [])
        risk = review.get("risk_charge_review", {})
        errors.extend(risk.get("blockers") or [])
        if risk.get("result") == "documents_confirmation_required":
            if not approval_options.get("risk_contract_confirmed"):
                errors.append("风险代理须确认已签专门书面风险代理合同")
            if not approval_options.get("risk_notice_confirmed"):
                errors.append("风险代理须确认已完成醒目告知和风险提示")
        fee = review.get("fee_reasonableness_review", {})
        if fee.get("result") in ("manual_review_required", "correction_required") and not (
            approval_options.get("fee_reviewed") and self._text(approval_options.get("fee_memo"))
        ):
            errors.append("收费存在异常提醒，须填写收费复核意见")
        return errors

    def _post_lian_approval(self, lawcase_id: int, approved: bool, memo: str) -> Any:
        if not self._http or not self._agent_api_ready:
            raise RuntimeError("AgentAPI 未登录")
        resp = self._http.post(
            f"{self.site_url}/DataServices/LawcaseSvr/Lianshenpi",
            data={"lId": str(lawcase_id), "isApproved": "true" if approved else "false", "memo": memo},
        )
        try:
            data = resp.json()
        except ValueError:
            data = {"text": resp.text}
        if resp.status_code != 200:
            raise RuntimeError(f"审批接口 HTTP {resp.status_code}: {data}")
        if isinstance(data, dict) and (data.get("Type") or data.get("Message")):
            raise RuntimeError(self._text(data.get("Message") or data))
        return data

    def _get_case_entity_for_approval(self, lawcase_id: int, detail: dict[str, Any]) -> dict[str, Any] | None:
        base_type = str(detail.get("baseTypeName") or detail.get("baseType") or "")
        owners = ["Page:LawcaseDetails_刑事@1"] if "刑事" in base_type else ["Page:LawcaseDetails_民事@1"]
        return self._get_entity("ApplicationData.Lawcase", [lawcase_id], owners)

    def _get_case_list_all(self, keyword: str) -> list[dict[str, Any]]:
        rows: list[dict[str, Any]] = []
        page_index = 0
        while True:
            payload = self._agent_get("GetLawcases", keyword=keyword, pageIndex=page_index, pageSize=100)
            if not isinstance(payload, dict):
                return rows
            page = payload.get("data") or []
            rows.extend(row for row in page if isinstance(row, dict))
            if not page or len(rows) >= int(payload.get("total") or 0):
                return rows
            page_index += 1

    def _approval_party_names(self, detail: dict[str, Any]) -> tuple[list[str], list[str]]:
        clients = [row for row in (detail.get("clients") or []) if isinstance(row, dict)]
        principals = [str(row.get("name") or "").strip() for row in clients if row.get("roleType") == 0]
        opponents = [str(row.get("name") or "").strip() for row in clients if row.get("roleType") == 1]
        return (
            [name for name in principals if name] or self._split_names(detail.get("wtrNames") or detail.get("dsrNames")),
            [name for name in opponents if name] or self._split_names(detail.get("tosNames")),
        )

    def _is_risk_charge(self, detail: dict[str, Any], entity: dict[str, Any]) -> bool:
        method_name = str(detail.get("chargeMethodName") or "")
        method = entity.get("chargeMethd")
        method_id = method.get("Id") if isinstance(method, dict) else method
        return "风险" in method_name or str(method_id or "") == str(RISK_CHARGE_METHOD_ID)

    def _risk_fee_cap(self, amount: Decimal | None) -> Decimal | None:
        if amount is None or amount <= 0:
            return None
        remaining = amount
        cap = Decimal("0")
        for width, rate in RISK_FEE_TIERS:
            portion = remaining if width is None else min(remaining, width)
            cap += portion * rate
            remaining -= portion
            if remaining <= 0:
                break
        return cap.quantize(Decimal("0.01"))

    def _read_monitor_state(self, path: Path) -> dict[str, Any]:
        try:
            return json.loads(path.read_text()) if path.exists() else {}
        except Exception:
            return {}

    def _row_id(self, row: dict[str, Any]) -> str:
        value = row.get("id") or row.get("lawcaseId")
        return str(value) if value is not None else ""

    def _require_lawcase_id(self, value: Any) -> int:
        try:
            case_id = int(value)
        except (TypeError, ValueError):
            raise RuntimeError("缺少 OA 案件 ID")
        if case_id <= 0:
            raise RuntimeError("OA 案件 ID 无效")
        return case_id

    def _split_names(self, value: Any) -> list[str]:
        if not value:
            return []
        values = value if isinstance(value, list) else re.split(r"[、,，;；/\n]+", str(value))
        return [name.strip() for name in values if str(name).strip()]

    def _normalize_name(self, value: str) -> str:
        return re.sub(r"\s+", "", unicodedata.normalize("NFKC", value or "")).casefold()

    def _decimal_value(self, value: Any) -> Decimal | None:
        if value in (None, ""):
            return None
        try:
            return Decimal(str(value))
        except (InvalidOperation, ValueError):
            return None

    def _decimal_to_float(self, value: Decimal | None) -> float | None:
        return float(value) if value is not None else None

    def _float_or_none(self, value: Any) -> float | None:
        parsed = self._decimal_value(value)
        return self._decimal_to_float(parsed)

    def _now_iso(self) -> str:
        return time.strftime("%Y-%m-%dT%H:%M:%S%z")

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
