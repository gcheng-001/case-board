"""OA 脚本抽象基类。所有 OA 适配器继承此类。

设计原则:
  - 登录/cookie 管理在基类,子类只实现具体业务逻辑
  - 进度通过 stdout JSON 行回报给 Rust 端
  - 失败必须抛异常,由 cli.py 统一捕获
"""
from __future__ import annotations

import json
import sys
import time
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


@dataclass
class OAResult:
    """OA 操作结果。"""
    success: bool
    message: str
    data: dict[str, Any] = field(default_factory=dict)


class OAScriptBase(ABC):
    """OA 脚本基类。子类必须实现 run_* 方法。"""

    def __init__(
        self,
        site_url: str,
        account: str,
        password: str,
        cookies_dir: str | None = None,
    ) -> None:
        self.site_url = site_url.rstrip("/")
        self.account = account
        self.password = password
        self.cookies_dir = Path(cookies_dir or Path.home() / ".caseboard" / "oa_cookies")
        self.cookies_dir.mkdir(parents=True, exist_ok=True)
        self._page: Any = None
        self._context: Any = None
        self._browser: Any = None
        self._playwright: Any = None

    # ─────────── 进度回报 ───────────

    def report(self, event: str, pct: int = 0, message: str = "", data: dict | None = None) -> None:
        """向 stdout 写一行 JSON 进度,供 Rust 端读取。"""
        payload = {
            "event": event,
            "pct": pct,
            "message": message,
        }
        if data:
            payload["data"] = data
        print(json.dumps(payload, ensure_ascii=False), flush=True)

    # ─────────── Cookie 管理 ───────────

    @property
    def _cookies_path(self) -> Path:
        safe_name = self.site_url.replace("https://", "").replace("http://", "").replace("/", "_")
        return self.cookies_dir / f"{safe_name}.json"

    def _save_cookies(self, cookies: list[dict]) -> None:
        self._cookies_path.write_text(json.dumps(cookies, indent=2, ensure_ascii=False))

    def _load_cookies(self) -> list[dict] | None:
        if not self._cookies_path.exists():
            return None
        try:
            cookies = json.loads(self._cookies_path.read_text())
            now = time.time()
            valid = [c for c in cookies if c.get("expires", -1) == -1 or c.get("expires", 0) > now]
            return valid if valid else None
        except (json.JSONDecodeError, OSError):
            return None

    # ─────────── Playwright 浏览器管理 ───────────

    def _ensure_browser(self, headless: bool = True) -> None:
        """确保 Playwright 浏览器已启动。"""
        if self._page is not None:
            return
        from playwright.sync_api import sync_playwright
        self._playwright = sync_playwright().start()
        self._browser = self._playwright.chromium.launch(headless=headless)
        self._context = self._browser.new_context()
        # 尝试加载 cookies
        cookies = self._load_cookies()
        if cookies:
            self._context.add_cookies(cookies)
        self._page = self._context.new_page()

    def _close_browser(self) -> None:
        if self._context:
            try:
                self._save_cookies(self._context.cookies())
            except Exception:
                pass
            self._context.close()
        if self._browser:
            self._browser.close()
        if self._playwright:
            self._playwright.stop()
        self._page = None
        self._context = None
        self._browser = None
        self._playwright = None

    # ─────────── 子类必须实现 ───────────

    @abstractmethod
    def login(self) -> bool:
        """登录 OA 系统。成功返回 True。"""
        ...

    @abstractmethod
    def run_filing(self, case_data: dict[str, Any]) -> OAResult:
        """执行 OA 立案。case_data 来自案件看板的案件数据。"""
        ...

    @abstractmethod
    def run_case_import(self) -> OAResult:
        """从 OA 导入案件列表。"""
        ...

    @abstractmethod
    def run_client_import(self) -> OAResult:
        """从 OA 导入客户列表。"""
        ...

    # ─────────── 统一执行入口 ───────────

    def execute(self, action: str, **kwargs: Any) -> OAResult:
        """统一入口:login → run_* → close。"""
        try:
            self.report("starting", 0, f"正在登录 {self.site_url}...")
            if not self.login():
                return OAResult(success=False, message=getattr(self, "_login_error", "登录失败"))
            self.report("logged_in", 10, "登录成功")

            if action == "filing":
                return self.run_filing(kwargs.get("case_data", {}))
            elif action == "case_import":
                return self.run_case_import()
            elif action == "client_import":
                return self.run_client_import()
            else:
                return OAResult(success=False, message=f"未知操作: {action}")
        except Exception as e:
            self.report("failed", 0, str(e))
            return OAResult(success=False, message=str(e))
        finally:
            self._close_browser()
