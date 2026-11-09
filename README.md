# Rust proto-gen

PHP、TypeScript、Rust、Go 共用的协议生成器。只接受新版 `common/api_options.proto`。
元数据解析、校验、字段索引、路由、控制器补全、产物安装和 OpenAPI 均由 Rust 实现。
消息绑定继续使用 protoc PHP、ts-proto、prost-build 和 protoc-gen-go 的既有输出契约。

## 使用

```bash
GENERATOR=$(sh install.sh v0.4.2 .tools/proto-gen)
"$GENERATOR" generate --config ../node-core/proto/generator.toml
"$GENERATOR" generate --config ../node-core/proto/generator.toml --check
"$GENERATOR" generate --config ../node-core/proto/generator.toml --force
"$GENERATOR" openapi --config ../node-core/proto/generator.toml --out /tmp/openapi.json
```

业务仓库先显式执行 `./tools/proto/prepare.sh`，按 `proto/proto-gen.version` 下载 GitHub Release，自动选择
macOS/Linux x86_64/ARM64，核对 SHA256SUMS 并缓存。缓存命中时无需联网。
日常生成只读取本地缓存，缓存缺失时直接报错，不访问 GitHub。不需要 Rust/Cargo，
也不依赖相邻源码仓库；PROTO_GEN_BIN 可显式覆盖。Docker 把项目挂载至 /code 后，
缓存保留在宿主机项目目录，容器重启或重建不重复下载。
生成不重启服务，不自动同步远端文档。

## Web 工作台

```bash
# 在业务仓库启动；支持重复 --config 管理多个项目
proto-gen serve --config proto/generator.toml --listen 127.0.0.1:7337
```

浏览器访问 http://127.0.0.1:7337，支持协议搜索、标签筛选、参数和响应查看、标题/标签编辑、
保存并生成、JSON/GET/multipart 请求调试、环境变量和 Token/Bearer 认证，以及单个或批量 Apifox 预览/同步。
HTML/CSS/JS 内置于二进制，不需要前端构建或联网加载资源。
使用说明见 [Web 工作台](docs/web-workbench.md)。

## Release 与工具链

推送与 Cargo.toml 版本一致的 v* 标签，GitHub Actions 先运行测试和 Clippy，
再为四个平台构建、执行独立工具包 smoke test，并发布带 SHA256SUMS 的 Release。
Linux 为 musl 静态链接。包内包含 proto-gen、protoc 35.0、protoc-gen-go 1.36.6、
公共 schema 和版本标记。参考 xcwen/ac-php 的 ac-php-mago-tags 发布矩阵。

编译器查找顺序为 PROTOC、二进制同目录的 protoc、开发源码的 toolchain/bin/protoc、
PATH。Go 插件优先使用目标项目 .tools/protoc-gen-go，否则使用工具包自带插件。
TypeScript 继续使用目标项目 node_modules/.bin/protoc-gen-ts_proto 2.7.0。
项目配置统一读取 proto/config.json，不执行 PHP 配置。

源码开发可使用 ./bin/proto-gen（需要 Rust/Cargo）；tools/install-protoc.sh 安装
固定版本编译器。Rust 依赖由 Cargo.lock 固定，GitHub 构建使用 Rust 1.97.0。
Docker 中也可由项目入口下载对应 Linux 包，无需在容器中编译 Rust。

## 配置与协议

```toml
version = 1
target = "php" # php / typescript / rust / go
profile = "php-coroutine"
source = "src"
output = "../src/gen/Proto"
controllers = "../src/App/Controllers"
# Rust/Go 还需 registry = "相对于本配置文件的注册文件路径"
```

所有路径相对于 generator.toml。schema 副本必须与本工具的
`schema/common/api_options.proto`、`field_options.proto` 完全一致；更新时统一同步各仓库。
协议目录、文件名、消息名称、字段编号和类型决定原有路由/类型映射。
`http` 通常省略：PHP/Node 不新增方法限制；Rust/Go 保持现有 POST 默认规则。
PRIVATE 仅允许具备对应认证实现的 PHP 目标。

`title` 和 `auth` 必填；新接口的默认标题可使用完整接口路径，鉴权默认写 SESSION，
显式 HTTP 方法使用大写枚举。说明字符串及外部 Markdown 原样保留。
字段元数据使用 `common.field`，文档模板使用 `common.doc_fields`，不再解释 CFG/ENUM 注释。
字段可省略必须显式声明 optional；集合沿用可省略语义，oneof 保持互斥。
默认值不决定 required，也不会填充运行时参数。nullable 独立控制 JSON null。
所有 64 位整数的 JSON 表示统一为字符串，已删除 json_int64 数字策略。
详见 [字段及配置契约](docs/field-and-project-options.md)、[Apifox 审查及实施说明](docs/apifox-audit-and-design.md)。

## 安装与检查

先在临时项目生成消息、路由及所有控制器变更，解析/编译失败时不写入业务仓库。
已有控制器只补缺失方法，Rust 使用 syn，其他目标使用 Tree-sitter。已有方法体
保持不变。文件使用临时文件原子替换；安装失败时回滚已安装文件。
`proto/.proto-gen-manifest.json` 记录管理范围，删除协议后只清理对应生成物，
保留手写控制器及其他生成器管理的 DB/枚举文件。

默认按内容指纹增量生成：输入、产物及控制器均未变化时跳过编译；有变化时仍全量校验协议，
PHP/TypeScript/Go 仅重新生成变更 Proto 及其依赖影响范围内的消息绑定，其他绑定校验后复用。
Rust 当前整体复用消息绑定，消息结构变更时整体重建；路由、字段索引和注册表始终来自完整模型。
缓存位于 `proto/.proto-gen-cache.json`，并使用 `proto/.proto-gen.lock` 防止并发安装；
两者应加入业务仓库 `.gitignore`。缓存缺失或损坏时自动重建，不需要手动准备。
`--force` 忽略缓存并重建全部绑定。具体行为和验证方式见 [增量生成](docs/incremental-generation.md)。

`--check` 在隔离目录重新生成并对比，不写回项目产物或控制器。
OpenAPI 导出不发请求；sync-apifox 只有显式使用 --dry-run 才不发请求，实际同步
先校验全部输入及目标，不打印凭据。生成阶段也不会输出配置中的凭据。

```bash
cargo fmt --all --check
cargo check --locked
cargo test --locked
```

测试覆盖：新版选项/optional presence、拒绝旧注释、HTTP 默认值、JSON 整数策略、
PHP/TS/Go 控制器保护和增量补全、Rust 注册、只读检查、失败不安装及文档转换。
源码结构：compiler.rs 读取统一模型，fields.rs 生成字段索引，targets.rs 输出各语言，
controller_edit.rs/controllers.rs 补全控制器，project.rs 负责安装，openapi.rs 生成并校验完整 OpenAPI 3.1 文档，apifox.rs 构造发布计划并检查官方导入 API 结果。

# Editor navigation index

Project generation also writes `proto/navigation.json` (beside the protocol source
directory). This versioned JSON contract maps logical endpoint identities to
protocol sources and editable controller declarations for PHP, TypeScript, Rust
and Go. Paths are relative to the project root inferred by `generator.toml`;
they never contain temporary build directories or developer-specific roots.

```json
{
  "version": 1,
  "endpoints": [{
    "id": "root/test/user_list",
    "method": "GET",
    "paths": ["/root/test/user_list"],
    "proto": "proto/src/root/test__user_list.proto",
    "controller": {
      "file": "internal/controllers/root/test.go",
      "owner": "TestController",
      "symbol": "UserList"
    }
  }]
}
```

`id` retains the source group/controller/action even when `http.path` overrides
the URL. `paths` lists only routes emitted by the selected target, including PHP
`_uri_no_project` aliases; custom HTTP paths do not imply an additional default
route. An unspecified method is normalized to `POST`. Workspace/project identity
is supplied by the consumer so separate checkouts can reuse the same index.
Rust raw identifiers are retained in `controller.symbol`.

The index is sorted by identity and method, included in `.proto-gen-manifest.json`,
installed transactionally with other generated artifacts, repaired by incremental
generation if deleted or edited, and verified without writes by `generate --check`.
Consumers should locate the symbol inside its owner instead of relying on stale
line numbers. Run `cargo test --locked --test navigation` for contract tests.
