# 贡献指南

> 欢迎为 **案件看板 · CaseBoard** 贡献代码、文档、反馈。

本项目是一线诉讼律师主导的开源工具,首要目标是好用,不是大而全。
贡献之前请先读 [README](./README.md) 和 [AGENTS.md](./AGENTS.md) 了解项目定位与铁律。

---

## 上手

```bash
# 1. fork → clone
git clone https://github.com/<你的用户名>/case-board.git
cd case-board

# 2. 装依赖
pnpm install

# 3. 跑 dev
pnpm tauri dev

# 4. 后端编译检查
cd src-tauri
cargo check
cargo clippy -- -D warnings
```

依赖:Rust ≥ 1.85,Node ≥ 20,pnpm ≥ 9,macOS 13+。

## 提交规范

使用 [Conventional Commits](https://www.conventionalcommits.org/zh-hans/v1.0.0/):

```
<type>(<scope>): <subject>

[optional body]

[optional footer]
```

常用 type:

| type | 用法 |
|:---|:---|
| `feat` | 新功能 |
| `fix` | bug 修复 |
| `docs` | 文档变更 |
| `style` | 不影响代码运行的格式调整 |
| `refactor` | 重构(既非新功能也非 bug 修复) |
| `perf` | 性能优化 |
| `test` | 测试相关 |
| `chore` | 构建、依赖、工具链 |
| `ci` | CI 配置变更 |

示例:

```
feat(scanner): 识别 AI 产物文件(总览/调查/精要)
fix(import): 处理 macOS 文件夹访问权限拒绝时的错误提示
docs: 补 V0.2 MCP server 设计说明
```

## 分支策略

- `main`: 稳定分支,只接受合并 PR
- 功能分支:`feat/xxx`、`fix/xxx`、`docs/xxx`

请基于最新 `main` 创建分支,PR 提交时确保通过 CI(`cargo check` / `cargo clippy` / `pnpm build`)。

## PR 流程

1. fork 仓库,基于 `main` 拉新分支
2. 提交代码,follow 上面的 commit 规范
3. 推到你的 fork,提 PR 到本仓库 `main`
4. PR 描述请用 PR 模板填写(背景 / 改动 / 测试 / 截图)
5. 等 CI 绿灯 + reviewer 通过后合入

**大改动请先开 issue 讨论再写代码**,避免做完合不进来。

## 代码风格

- Rust: `cargo fmt` + `cargo clippy`,任何 clippy warning 都要修
- TypeScript: Prettier 默认 + 项目 `.prettierrc`
- 文件命名:Rust 用 `snake_case`,TS 用 `camelCase` 文件名 + `PascalCase` 组件名
- 中文 ↔ 英文混排时,中英文之间留一个空格(`案件 ID` 不是 `案件ID`)

## 隐私铁律

**永远不要 commit 真实当事人数据**:案件名、当事人姓名、案号、身份证号、电话、地址、聊天记录截图等。
本公开仓**不收录任何测试代码与样例案件数据**(维护者在私有环境另行测试);PR 请勿附带测试 fixture 或案例数据,无论真实还是虚构。

PR 里如果发现真实数据,Reviewer 必须直接拒绝。

## 问题反馈

- bug:用 [bug report 模板](./.github/ISSUE_TEMPLATE/bug_report.yml)
- 功能建议:用 [feature request 模板](./.github/ISSUE_TEMPLATE/feature_request.yml)
- 安全漏洞:**不要公开发 issue**,见 [SECURITY.md](./SECURITY.md)

## 行为准则

参与本项目即表示同意遵守 [行为准则](./CODE_OF_CONDUCT.md)。

## License

本项目以 [PolyForm Noncommercial License 1.0.0](./LICENSE) 授权(非商用免费,商用须取得版权人书面授权)。

提交贡献即表示你同意:你的贡献同样以 PolyForm Noncommercial License 1.0.0 授权,并**授予版权人(刘成 / 江苏漫修律师事务所)以其他条款(含商业授权)对你的贡献再许可的权利** —— 这是项目能对外提供商业授权所必需的。
