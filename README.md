# Rust proto-gen

PHP、TypeScript、Rust、Go 共用的协议生成器。只接受新版 `common/api_options.proto`。
元数据解析、校验、字段索引、路由、控制器补全、产物安装和 OpenAPI 均由 Rust 实现。
消息绑定继续使用 protoc PHP、ts-proto、prost-build 和 protoc-gen-go 的既有输出契约。

## 使用

```bash
GENERATOR=$(sh install.sh v0.1.0 .tools/proto-gen)
"$GENERATOR" generate --config ../node-core/proto/generator.toml
"$GENERATOR" generate --config ../node-core/proto/generator.toml --check
"$GENERATOR" openapi --config ../node-core/proto/generator.toml --out /tmp/openapi.json
```

业务仓库的入口按 `proto/proto-gen.version` 下载 GitHub Release，自动选择
macOS/Linux x86_64/ARM64，核对 SHA256SUMS 并缓存。缓存命中时无需联网。
日常生成不需要 Rust/Cargo，也不依赖相邻源码仓库；PROTO_GEN_BIN 可显式覆盖。
生成不重启服务，不自动同步远端文档。

## Release 与工具链

推送与 Cargo.toml 版本一致的 v* 标签，GitHub Actions 先运行测试和 Clippy，
再为四个平台构建、执行独立工具包 smoke test，并发布带 SHA256SUMS 的 Release。
Linux 为 musl 静态链接。包内包含 proto-gen、protoc 35.0、protoc-gen-go 1.36.6、
公共 schema 和版本标记。参考 xcwen/ac-php 的 ac-php-mago-tags 发布矩阵。

编译器查找顺序为 PROTOC、二进制同目录的 protoc、开发源码的 toolchain/bin/protoc、
PATH。Go 插件优先使用目标项目 .tools/protoc-gen-go，否则使用工具包自带插件。
TypeScript 继续使用目标项目 node_modules/.bin/protoc-gen-ts_proto 2.7.0。
PHP 项目配置为 PHP 文件时仅调用 PHP 读取配置，不调用 PHP 协议生成器。

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
`schema/common/api_options.proto` 完全一致；更新时统一同步各仓库。
协议目录、文件名、消息名称、字段编号和类型决定原有路由/类型映射。
`http` 通常省略：PHP/Node 不新增方法限制；Rust/Go 保持现有 POST 默认规则。
PRIVATE 仅允许具备对应认证实现的 PHP 目标。

`title` 和 `auth` 必填；新接口的默认标题可使用完整接口路径，鉴权默认写 SESSION，
显式 HTTP 方法使用大写枚举。说明字符串及外部 Markdown 原样保留，CFG 配置继续由
descriptor 中的字段注释生成。`json_int64: JSON_INT64_NUMBER` 在 Rust/Go 中保留
64 位整数的 JSON number 策略，其余接口继续沿用运行时默认序列化。

## 安装与检查

先在临时项目生成消息、路由及所有控制器变更，解析/编译失败时不写入业务仓库。
已有控制器只补缺失方法，Rust 使用 syn，其他目标使用 Tree-sitter。已有方法体
保持不变。文件使用临时文件原子替换；安装失败时回滚已安装文件。
`proto/.proto-gen-manifest.json` 记录管理范围，删除协议后只清理对应生成物，
保留手写控制器及其他生成器管理的 DB/枚举文件。

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
controller_edit.rs/controllers.rs 补全控制器，project.rs 负责安装，apifox.rs 生成文档。
