"""OA 脚本注册表。按 OA 类型派发到对应适配器。"""
from __future__ import annotations

from typing import Type

from .base import OAScriptBase
from .nedev import NedevScript

# OA 类型 → 适配器类
_REGISTRY: dict[str, Type[OAScriptBase]] = {
    "nedev": NedevScript,
    # 未来可扩展:
    # "jtn": JtnScript,
    # "generic": GenericWebScript,
}


def get_script_class(oa_type: str) -> Type[OAScriptBase]:
    """按 OA 类型获取适配器类。"""
    cls = _REGISTRY.get(oa_type)
    if cls is None:
        raise ValueError(f"不支持的 OA 类型: {oa_type}。支持: {list(_REGISTRY.keys())}")
    return cls


def create_script(oa_type: str, site_url: str, account: str, password: str, **kwargs) -> OAScriptBase:
    """创建 OA 脚本实例。"""
    cls = get_script_class(oa_type)
    return cls(site_url=site_url, account=account, password=password, **kwargs)
