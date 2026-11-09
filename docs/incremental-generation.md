# 按变更生成

日常命令保持不变：

```sh
proto-gen generate --config proto/generator.toml
proto-gen generate --config proto/generator.toml --force
proto-gen generate --config proto/generator.toml --check
```

默认生成记录输入和产物的 BLAKE3 内容指纹。输入、受管理产物及控制器均未变化时，
直接返回 `unchanged (compilation skipped)`，不调用 protoc 或消息插件。
`--force` 忽略缓存，重建所有生成单元；`--check` 始终在隔离目录全量生成并比较，
不写产物、控制器、manifest、缓存或锁文件。

## 增量范围

有输入变化时，目前仍执行两轮完整 descriptor 编译、完整元数据校验和路由冲突检查。
随后按剥离自定义选项后的 Proto 内容及传递 import 依赖，确定消息绑定是否需要重建。

- PHP、Go：每个 Proto 为独立生成单元，公共类型变化会使依赖它的单元失效。
- TypeScript：每个接口 Proto 为独立生成单元，保留原有 ts-proto 参数及后处理规则。
- Rust：所有 prost 消息绑定为一个生成单元；结构变化整体重建，纯接口元数据变化可以复用。
- 接口元数据、外部说明、字段约束、枚举目录变化仍更新完整模型和相关汇总产物。
  如果剥离后的绑定输入未变，则不调用消息插件。
- 路由、字段索引、命令清单、descriptor 和注册表使用全部接口生成。
- 已有控制器继续校验并补充缺失方法，保留手写方法体。控制器被删除或修改会使快速跳过失效。

缓存输入包括源目录文件（含 Markdown）、生成配置、项目 JSON 配置、枚举目录、
生成器二进制、解析出的 protoc 路径及内容，以及相关语言工具。
TypeScript 额外跟踪插件实现目录、Node 和项目依赖锁文件；Go 跟踪插件、go.mod 和 gofmt。
环境指纹只保存哈希，不保存环境变量明文。

输出日志报告本轮编译和复用的消息绑定单元数量。例如：

```text
php: 20 interfaces, 3 changed files, 1 binding units compiled, 22 reused
```

缓存首次建立或失效时，会逐单元调用插件以准确记录产物归属，因此首次生成可能比原来的
批量调用更慢。日常生成复用未受影响单元；当前尚未实现 descriptor 按文件增量编译，
也未实现 Rust 按 package 增量生成。

## 产物、删除与失败处理

`proto/.proto-gen-manifest.json` 继续保存完整产物清单。
`proto/.proto-gen-cache.json` 记录版本、输入指纹、生成单元与产物指纹，不保存绑定内容副本。
复用绑定前会核对已安装文件；文件缺失或内容被修改时重新生成所属单元。
复用产物也会进入暂存目录和完整 manifest，不会被误判为删除对象。
删除/重命名协议后，仅清理不再属于任何生成单元或汇总步骤的产物，保留手写控制器。

所有生成和校验先在临时目录完成。安装前再次核对输入、已有产物和控制器，
遇到外部并发修改时中止并提示重试。生成进程使用文件锁防止 CLI/Web 同时安装。
产物、manifest 和缓存参与同一次安装及失败回滚，编译或安装失败不推进缓存。
缓存格式损坏或版本不匹配时自动重建；也可删除缓存或使用 `--force` 排查。

业务仓库应忽略以下两个本地文件，继续按原有规则管理 manifest：

```gitignore
proto/.proto-gen-cache.json
proto/.proto-gen.lock
```

## 验证

```sh
cargo fmt --all --check
cargo check --locked
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
```

`tests/incremental.rs` 通过真实 protoc 及调用日志验证无变更跳过、元数据复用、
传递依赖失效、删除清理、产物修复、控制器保护、缓存损坏恢复、工具变更、失败回滚，
并比较增量、`--check` 批量全量生成和 `--force` 的产物一致性。
完整 Go 测试需要 PATH 中有 gofmt。可额外提供真实插件执行 TypeScript/Go 增量测试：

```sh
PROTO_GEN_TEST_NODE_MODULES=/absolute/project/node_modules \
PROTO_GEN_TEST_GO_PLUGIN=/absolute/path/protoc-gen-go \
cargo test --locked --test incremental
```

未提供对应路径时，两个可选插件测试直接跳过；PHP/Rust 回归测试始终运行。
