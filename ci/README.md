# PostgreSQL CI

[postgres.yml](postgres.yml) 是完整的 GitHub Actions 模板，覆盖 PostgreSQL 16 / 18、格式检查、Clippy、数据库测试和宏编译 fixture。

当前自动工作流尚未启用。将模板移到 `.github/workflows/ci.yml` 后提交，即可在 push / pull_request 时运行：

```sh
mkdir -p .github/workflows
git mv ci/postgres.yml .github/workflows/ci.yml
git commit -m "Enable PostgreSQL CI"
git push
```

上传工作流需要 GitHub 认证拥有相应的工作流写权限；使用 classic OAuth token 时需要 `workflow` scope。CI 仅使用临时 PostgreSQL 服务，不需要配置业务数据库凭据。启用后请同步更新 README 与验证记录中的 CI 状态。
