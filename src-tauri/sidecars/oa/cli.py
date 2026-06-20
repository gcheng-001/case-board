"""OA sidecar CLI 入口。

由 Rust 端通过 tokio::process::Command 调用,参数:
  --action      filing / case_import / client_import
  --session-id  会话 ID(用于进度回报)
  --site-url    OA 登录地址
  --account     登录账号
  --password    登录密码
  --oa-type     OA 平台类型(nedev / jtn / generic)
  --case-id     案件 ID(立案用,可选)
  --case-data   案件数据 JSON(立案用,可选)

所有进度通过 stdout JSON 行回报给 Rust 端。
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

try:
    from .registry import create_script
except ImportError:
    # Rust launches this file directly, so relative imports do not have a
    # package context. Add the sidecars directory and import through `oa`.
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
    from oa.registry import create_script


def main() -> None:
    parser = argparse.ArgumentParser(description="CaseBoard OA sidecar")
    parser.add_argument("--action", required=True, choices=["filing", "case_import", "client_import"])
    parser.add_argument("--session-id", required=True)
    parser.add_argument("--site-url", required=True)
    parser.add_argument("--account", required=True)
    parser.add_argument("--password", required=True)
    parser.add_argument("--oa-type", default="nedev")
    parser.add_argument("--case-id", default=None)
    parser.add_argument("--case-data", default="{}")
    args = parser.parse_args()

    # 解析案件数据
    try:
        case_data = json.loads(args.case_data) if args.case_data else {}
    except json.JSONDecodeError:
        case_data = {}

    # 创建脚本
    script = create_script(
        oa_type=args.oa_type,
        site_url=args.site_url,
        account=args.account,
        password=args.password,
    )

    # 执行
    if args.action == "filing":
        result = script.execute("filing", case_data=case_data)
    elif args.action == "case_import":
        result = script.execute("case_import")
    elif args.action == "client_import":
        result = script.execute("client_import")
    else:
        result = None

    # 输出最终结果
    if result:
        print(json.dumps({
            "event": "completed" if result.success else "failed",
            "pct": 100 if result.success else 0,
            "message": result.message,
            "data": result.data,
        }, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
