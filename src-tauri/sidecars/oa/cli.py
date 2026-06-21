"""OA sidecar CLI 入口。

由 Rust 端通过 tokio::process::Command 调用,参数:
  --action      filing / case_import / client_import / pending_approvals /
                approval_check / approval_approve / approval_reject /
                approval_monitor_snapshot / download_engagement_documents /
                resolve_engagement_lawcase
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
    parser.add_argument(
        "--action",
        required=True,
        choices=[
            "filing",
            "case_import",
            "client_import",
            "pending_approvals",
            "approval_check",
            "approval_approve",
            "approval_reject",
            "approval_monitor_snapshot",
            "download_engagement_documents",
            "resolve_engagement_lawcase",
        ],
    )
    parser.add_argument("--session-id", required=True)
    parser.add_argument("--site-url", required=True)
    parser.add_argument("--account", required=True)
    parser.add_argument("--password", required=True)
    parser.add_argument("--oa-type", default="nedev")
    parser.add_argument("--case-id", default=None)
    parser.add_argument("--case-data", default="{}")
    parser.add_argument("--lawcase-id", type=int, default=None)
    parser.add_argument("--memo", default="")
    parser.add_argument("--confirm", action="store_true")
    parser.add_argument("--approval-options", default="{}")
    parser.add_argument("--monitor-state-path", default=None)
    parser.add_argument("--output-dir", default=None)
    args = parser.parse_args()

    # 解析案件数据
    try:
        case_data = json.loads(args.case_data) if args.case_data else {}
    except json.JSONDecodeError:
        case_data = {}

    try:
        approval_options = json.loads(args.approval_options) if args.approval_options else {}
    except json.JSONDecodeError:
        approval_options = {}

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
    elif args.action == "pending_approvals":
        result = script.execute("pending_approvals")
    elif args.action == "approval_check":
        result = script.execute(
            "approval_check",
            lawcase_id=args.lawcase_id,
            approval_options=approval_options,
        )
    elif args.action == "approval_approve":
        result = script.execute(
            "approval_approve",
            lawcase_id=args.lawcase_id,
            memo=args.memo,
            confirm=args.confirm,
            approval_options=approval_options,
        )
    elif args.action == "approval_reject":
        result = script.execute(
            "approval_reject",
            lawcase_id=args.lawcase_id,
            memo=args.memo,
            confirm=args.confirm,
            approval_options=approval_options,
        )
    elif args.action == "approval_monitor_snapshot":
        result = script.execute(
            "approval_monitor_snapshot",
            approval_options=approval_options,
            monitor_state_path=args.monitor_state_path,
        )
    elif args.action == "download_engagement_documents":
        result = script.execute(
            "download_engagement_documents",
            lawcase_id=args.lawcase_id,
            output_dir=args.output_dir,
        )
    elif args.action == "resolve_engagement_lawcase":
        result = script.execute("resolve_engagement_lawcase", case_data=case_data)
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
