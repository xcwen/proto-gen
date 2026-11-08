# Apifox 额外配置审查与改造方案

审查范围：五个仓库的 997 个接口、公共类型、字段注释、接口选项、项目导出配置、Rust 导出器、客户端配置消费及本地导出。源文件共检索到 401 处 CFG 标注和 37 处 ENUM 标注（含公共定义和被注释的旧字段，数量不等于活跃请求字段数）。本次做源码检查、本地导出和 dry-run；远端项目的手工配置未读取。本报告记录改造前审查与方案；现已按用户后续约定实施。最新字段、配置与同步契约见 field-and-project-options.md。

用户约定：取消手写 required 配置。可省略字段显式写 optional；OpenAPI 必填列表从 Proto 结构推导，default 不参与必填判断。当前项目地址归属、跨仓发布地址重复及凭据存放方式按现状处理，不纳入本轮整改。

## 1. 确认的问题

1. job__list.limit 的 default=10 目前只生成文档。后端 Job.php 的 list 方法自行将 0 替换为 10，并拒绝大于 500。普通 uint32 不区分未传与显式 0；若业务允许省略，按新约定应声明 optional，保留当前后端缺省处理。
2. Apifox message_schema 根据有无 default 决定 required，忽略 optional。Node 的 device_opt 虽声明 optional virtual_flag，导出仍被标为必填。
3. resource_id_list 的数组 default/examples 被写到 items，且数组被输出为 type:["array",null]；这里 null 不是合法类型名称，也不能由有默认值推导出可空。
4. CFG.enum 的 4 处配置未被 Apifox 导出器读取；ENUM 标注也未解析。出现 1 处未知 CFG.xx，当前静默忽略。
5. CFG.from 的 18 处引用只复制说明，不继承公共定义的默认值、示例或其他配置。当前按 common_field_t 的短消息名查找，无明确命名空间和引用契约。
6. 模型 model 从公共定义获得说明后，未获得公共 default="ali"，又被旧规则认定必填。是否使用 ali 作为实际默认值需由业务确认，不能从文档载体推断。
7. 接口请求示例能导出；response_examples 虽生成到配置，响应文档未消费。示例只检查 JSON 语法，没有按最终 Schema 校验。
8. category_path 未用于目录，目录取 project 或“开发中”；maintainers 未输出官方责任人扩展。sync 给所有语言追加 rust 标签，aggregate openapi 另做替换，两条链路不同。
9. deprecated 仅映射到 x-apifox-status，未输出标准 deprecated；deprecation_message 未展示。
10. GET 仍输出 requestBody，SSE 仍输出普通 JSON 成功包装。枚举及 int64/uint64 的文档映射没有完整采用各运行时 JSON 策略。
11. 单接口 document 在没有 oneOf 时声明 OpenAPI 3.0.1，聚合导出则为 3.1.0；数组输出混用类型数组，单接口片段还缺完整文档的 info。
12. 只有 CFG 里有默认值才判可省略、输入和输出共用相同 required 规则，均需按新 optional 约定替换。
13. 主库 dry-run 实测报 invalid type: sequence, expected a map：PHP 的空 url_replace_cfg 为 []，Rust 按 Map 解码失败。只修读取适配，不整理用户现有配置内容。
14. 导入响应只检查 HTTP 状态及 success:true，未检查导入错误计数/错误结果；缺少测试项目 ID 时跳过后仍可报告成功。
15. 主库还有 t.sh、sync_apifox_test.sh 等旧入口引用已移除的 PHP 脚本；需要转发统一入口或清理失效入口。

## 2. 字段配置完整清单

| 当前配置 | 当前实现 | 新方案 |
| --- | --- | --- |
| 普通字段注释 | 取 descriptor 注释；字段索引和 Apifox 各有一套处理 | 统一解析一次，作为缺省说明 |
| CFG.default | 标量能导出；同时错误控制必填性；数组位置错误 | default，按字段完整类型校验，独立于 optional |
| CFG.example | 写为 examples；不检查类型，数组示例挂到 items | examples，按最终 Schema 校验，容器与元素分开 |
| CFG.from | 只替换说明，按 common_field_t 短名查找 | description_ref 只复用说明；template_ref 显式复用整套文档元数据 |
| CFG.field_type | 可任意覆盖 schema.type；file 特判为 binary | 受限的格式/上传配置，不能替换真实 Proto 类型 |
| CFG.enum | 保存进字段索引，但导出未读取 | 显式 enum_values 或 enum_ref，校验值类型与默认值 |
| ENUM=枚举名 | 未解析 | enum_ref，引用现有共享枚举的本地快照，生成时不联网 |
| CFG.xx 等未知键 | 静默忽略 | 迁移时修正/删除；新配置未知键报文件、字段和位置 |
| required | 没有独立可靠契约，由 default 推导 | 不提供该配置；只从 optional/集合/oneof 结构推导 |
| nullable | 仅数组被 default 间接错误赋予 | 独立声明，不由 optional 或 default 推断 |
| 类型与范围 | 多数标量仅给粗略 type | 类型来自 descriptor；补 unsigned、enum、bytes、map、整数 JSON 策略等确定映射 |
| 长度/数值/格式约束 | 尚无完整配置契约 | 注册白名单并按 Proto 类型校验，不根据字段名猜测格式 |

已观察到 uint32 示例写成字符串、bool 示例写成数字、不可空字段示例为 null 等问题。迁移时逐项核对，不将错误示例反向解释成新的字段类型。

## 3. 接口级选项完整清单

| 配置 | 处理方案 |
| --- | --- |
| title | summary，同时生成稳定 operationId；改标题不应改变接口身份 |
| category_path | 唯一目录来源，每项一层，正确处理斜杠转义 |
| tags | 标签；仅保留现有默认选目标规则，不隐式代表目录或鉴权 |
| maintainers | 输出官方 x-apifox-maintainer；主责任人选择规则明确，多人列表不能直接填入单字符串字段 |
| project | 保留项目/服务标识和 PROJECT:* 标签用途，不再覆盖目录 |
| auth | 保持运行时含义；文档认证默认继承项目设置，不因 PUBLIC 就强行取消其他中间件要求的认证 |
| http.method | 采用声明或既有项目默认值，不新增运行时方法限制；GET 文档正确使用 query |
| http.path | 内部路由映射与发布 URL 变换分层；发布前核对 method+最终路径 |
| http.request_content_type | requestBody 的媒体类型；文件上传对应 multipart，字段为 binary |
| description/description_file | 保留 Markdown 与换行，二选一；外部文件仍限定项目内 |
| examples.request_json | 作为完整请求 body 示例，并校验最终请求 Schema |
| examples.response_json | 明确完整响应 body 语义；如需 data 区示例须显式标作用域再按响应包装转换 |
| deprecated | 同时输出标准 deprecated:true 和 Apifox 状态 |
| deprecation_message | 纳入弃用说明，避免只存配置不展示 |
| export_targets | 使用现有目标名称引用；空配置、未知引用和无匹配应明确报错 |
| config_json.examples | 迁移到现有强类型 examples，不保留两个覆盖来源 |
| config_json.in_content_type | 迁移到现有 http.request_content_type |
| config_json.parameters | 纳入类型明确的参数定义，区分 query/header/path/cookie，不将认证头混入业务 body |
| config_json.status | 规范为接口文档状态，和 deprecated 的优先级明确 |
| config_json.response_examples | 接入真正的 response.content.examples，按响应 Schema 校验 |
| config_json.return_check | 属于业务运行时扩展，继续留在运行时域，不当作 Apifox 校验配置 |
| 其他 config_json 键 | 必须注册所属域及消费位置；禁止无提示存入后不生效 |
| command_id | 保持运行时接口编号语义，不当作 Apifox 文档选项 |
| json_int64 | 已移除，所有 64 位整数统一为 JSON 字符串 |

发布目标的实际 ID、凭据和地址归属按用户要求不整理。这里审查的是配置如何被程序读取和消费。

## 4. 项目与同步配置

| 配置/行为 | 审查结论与处理 |
| --- | --- |
| project_name/name | 显示名称和稳定引用键应区分，当前值不整理 |
| release/test project_id | 当前 17 个目标中 14 个未设 test ID；选中目标缺少当前环境 ID 时明确失败，避免“成功 0 次” |
| apifox_token | 保持现有来源；不在审查报告、日志及错误响应中输出值 |
| tag_list/export_targets | 保留默认标签匹配与显式目标匹配，校验后统一生成发布计划 |
| url_replace_cfg | 当前规则为 14 条 slash 正则；未知格式不能静默跳过；修复 PHP 空映射读取 |
| export_tags_flag | 只控制标签输出，不能影响目录、责任人、状态等其他扩展 |
| 分支/模块/环境 | 当前多为默认行为或写死；若提供额外配置则必须有明确消费映射，不擅改现有目的地 |
| 名称/路径覆盖策略 | 当前硬编码；应明确稳定身份、生成内容所有权与保留范围，避免新增 components 后仍忽略模型更新 |
| timeout/redirect | 保留有限超时和明确 HTTP 错误处理，不自动重试不确定的写入 |
| 导入响应 | 对照所用官方导入 API 的返回结构验证错误结果/计数，不只看 success |
| dry-run | 和真实同步共用文档及发布计划构造，仅替换传输层；零远端写入 |
| 生成/检查 | 继续只读缓存，禁止拉 GitHub；下载只通过 prepare.sh 显式执行 |

本轮不进行现有目标重新归属或凭据迁移。Rust 当前只有示例目标配置、Go 尚无实际目标配置，需要使用者按现有部署自行提供，不能凭空生成项目 ID。

## 5. OpenAPI 与客户端联动

- 统一输出完整 OpenAPI 3.1：info、paths、components、稳定 operationId；同步片段和本地导出调用同一个 builder。
- 共享真实消息（如 flow_line_data_item、ppt_job_simple_t）进入 components.schemas，以完整 Proto 名引用；允许递归模型，避免无限展开或仅因存在循环就拒绝。
- default/examples 放在字段自身；items 只承载元素 Schema；map 使用 additionalProperties。
- optional 控制可省略；nullable 控制出现时是否可为 null；default 只是文档注解；这三件事分开。
- request/response 按方向构造 Schema；输出字段存在性结合既有序列化规则，不能套用输入的默认值判断。
- 对不同 JSON 整数/枚举/bytes/WKT 表示采用已确认的项目序列化契约；不因 Proto 是 uint64 就一律输出 integer。
- 普通 JSON 包装、裸 JSON、文件、重定向和 SSE 单独表达。当前 Node 的 GET SSE 接口被导成 JSON body/response，必须纠正媒体类型与参数位置。
- 认证文档保留继承策略；Node 服务商约定继续由项目统一设置，不逐接口重复 Authorization 或写死 tenant-id。
- SDK 配置的 apifox_config、token_env、doc_base_url、tag、proto_list、class_name、server_name、default_host_url 也纳入契约检查；不调整当前配置值。
- 当前 TS 客户端有 body 时直接返回 body 类型，可能漏掉 query 参数；RPC 传输固定 POST。需要区分内部 RPC 约定与按 OpenAPI 方法发送的 HTTP 客户端，流式/文件操作不可生成普通 JSON 调用而不提示。

## 6. 建议的新 Proto 结构

```text
common/api_options.proto     # 接口级选项及明确的文档级选项
common/field_options.proto   # 字段元数据，禁止 required 配置
common/doc_fields.proto      # 文档模板注册，不是业务消息
common/common.proto          # 现有真实公共数据模型，包名/字段号保持
```

common_field_t 目前承担文档模板载体，本次检索未见五仓手写业务依赖该类型。待其引用完成迁移后移除载体，文档模板不再生成多余的业务类型。

示意（新选项需实现后才能使用）：

```proto
import "common/field_options.proto";
message in {
  optional string after = 1 [(common.field) = {
    default: ""
  }];
  optional uint32 limit = 2 [(common.field) = {
    default: "10"
    examples: "10"
    max_uint: 500
  }]; // 每页条数，省略或 0 时按 10 处理
}
```

不设 required，也不根据 default 改变必填性。普通标量/消息与显式 optional 的文档约定明确；repeated/map 不允许 optional，保持既有可省略语义。新接口如需严格集合 presence，可用单值 wrapper message 表达；不自动改变旧接口 JSON 形状。oneof 的互斥及分支条件从结构生成。

默认值及示例使用 JSON 文本并由 Rust 按字段类型校验，保留大整数精度及 0/false/空字符串/空对象/空数组/null 的区别。文档默认值不自动注入运行时。job.list 现有 0->10 和 >500 拒绝逻辑保留。

公共复用必须分清：description_ref 只共享说明；template_ref 显式复用文档默认值、示例和约束。字段名称、编号、类型及 optional 属性始终来自实际 Proto，不由模板改变。完整引用键唯一，检测不存在、重复、类型不兼容及引用环。本地显式字段选项覆盖模板，数组整体替换，显式清除与未配置分开。

## 7. 实施与验证

1. 确定强类型选项、配置白名单及上述 optional 规则，不新增 required 参数。
2. Rust 只解析一份字段模型，字段索引、OpenAPI、SDK 全部消费解析后的模型；删除 fields.rs/apifox.rs 两套 CFG 解释逻辑。
3. 逐项迁移当前 CFG/from/ENUM；未知 xx、错误类型示例和集合默认值先核对，不能静默转换或忽略；新版本只支持新选项。
4. 将 common_field_t 引用迁到文档模板；真实公共数据模型保持原有包名、字段号和业务含义。
5. 先修空映射读取、数组位置、optional、enum、响应示例、目录/责任人/状态、媒体类型及 JSON 格式，再统一完整文档 builder。
6. 对照原有行为做代表性回归：limit 未传/0/500/501，optional 未传与零值，false、空值、数组/map、文件、oneof、SSE，以及 Rust/Go 64 位数值策略。
7. 校验完整 OpenAPI、所有默认值/示例与 Schema、一致的 dry-run/同步文档和 SDK 类型；验证远端导入结果识别逻辑，真实发布仍由显式同步命令执行。

参考：
- https://protobuf.dev/programming-guides/field_presence/
- https://spec.openapis.org/oas/v3.1.0.html
- https://docs.apifox.com/import-openapi-swagger
- https://docs.apifox.com/x-apifox-folder
- https://docs.apifox.com/x-apifox-maintainer
