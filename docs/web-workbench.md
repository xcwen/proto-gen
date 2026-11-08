# ProtoGen Web 工作台

Web 界面随 v0.3.0 的二进制发布，前端资源全部内置。启动和查看协议不下载 GitHub 文件，不自动向 Apifox 发布。

## 启动

业务仓库先显式安装当前版本，然后运行：

```bash
./tools/proto/prepare.sh
./proto/web.sh
```

默认访问 http://127.0.0.1:7337。也可直接使用缓存的二进制：

```bash
.tools/proto-gen/v0.3.0/macos-aarch64/proto-gen serve \
  --config proto/generator.toml --listen 127.0.0.1:7337
```

多个项目共用同一界面时重复指定 --config：

```bash
proto-gen serve \
  --config /Users/jim/aigc-php-core/proto/generator.toml \
  --config /Users/jim/node-core/proto/generator.toml \
  --config /Users/jim/aigc-php-core-co/proto/generator.toml \
  --config /Users/jim/rust-core/proto/generator.toml \
  --config /Users/jim/go-core/proto/generator.toml
```

Linux 容器可以使用已下载的 linux-x86_64/linux-aarch64 缓存。需要从容器外访问时显式指定监听地址和口令环境变量：

```bash
# PROTO_GEN_WEB_TOKEN 由运行环境提供；不要把真实口令提交到配置文件。
./proto/web.sh --listen 0.0.0.0:7337 --token-env PROTO_GEN_WEB_TOKEN
```

非回环地址必须配置口令，浏览器进入时填写该口令。容器端口映射或反向代理由实际部署配置提供。

## 使用

- 左侧选择项目，按标题、路径、标签搜索；可以筛选标签并勾选多个接口。
- 接口文档直接使用现有 Rust OpenAPI 模型，展示 optional、默认值、示例、集合、枚举、嵌套类型和 oneof 分支。
- “编辑标题与标签”只修改原生 common.api 的 title/tags，保留其他选项、描述、字段和控制器方法。保存前检查源文件版本，使用编译器临时目录验证；保存后重新生成当前项目。生成失败会回退本次可回退的源修改，保留原生成物；外部编辑不会被强制覆盖。
- “生成代码”调用现有统一生成流程。外部修改协议后点击刷新即可重新读取；generator.toml 中项目路径改变后应重启工作台。
- 请求调试中填写真实服务地址，可包含 /api 或 route__ 前缀；接口路径与地址拼接后发送。Query、Headers 使用 JSON 对象。测试鉴权时在请求头中填写 Authorization、Cookie 或实际服务要求的头。
- JSON、GET/HEAD 查询参数、原始文本、multipart 文本及文件均可发送。文件字段名可编辑；multipart 的 Content-Type 边界自动生成。等待时间为 1–120 秒，请求最大 20 MiB，响应预览最多 2 MiB；SSE 可查看等待时间内收到的内容。重定向展示原响应，不自动跟随。响应体、响应头、状态码、耗时和实际请求 URL 均可查看，二进制响应可下载。
- 服务地址按项目保存在浏览器；请求内容、访问口令和调试请求头只保留在当前页面内存。工作台不会自动转发浏览器 Cookie 或自身访问口令到被测服务。
- Apifox 面板选择当前接口或批量选择、测试/正式项目以及目标。预览和同步复用 CLI 的同一发布计划，并检查页面所见协议和配置的版本；外部修改后须刷新，避免旧页面选中错误目标。未匹配标签、缺少目标 ID、重复发布路径等在远端写入前失败。预览不发送导入请求；点击同步才执行导入。部分成功时明确列出已完成项，并停止后续导入，不自动重试结果不确定的写入。

实际发布继续读取各仓库 proto/config.json 的现有目标配置。界面只返回目标名称、ID、标签和可用状态，Apifox 令牌留在服务端。协议源文件和源码预览按当前项目限定路径；写入接口检查同源会话。

## 验证

`cargo test --test web` 覆盖读取、保存并生成、冲突检测、失败回退、路径范围、访问口令、请求转发、multipart 和 Apifox 预览。发布包的 smoke test 会启动内置 Web 服务并读取协议列表。桌面/窄屏浏览器交互另外使用本机 Chrome 验证，不访问真实 Apifox 写入接口。
