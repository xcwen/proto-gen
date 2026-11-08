# 字段及配置契约（v0.2.0）

五个业务仓库使用同一 Rust 生成器，只支持新版选项。公共定义为 `common/api_options.proto`、`common/field_options.proto`；文件内通常不需要 http。

```proto
import "common/api_options.proto";
import "common/field_options.proto";
option (common.api) = {
  title: "列表"
  auth: SESSION
  category_path: "小程序"
  category_path: "公共"
  maintainers: "ryan"
};
message in {
  optional uint32 limit = 1 [(common.field) = {
    default: "10"
    examples: "10"
    max_uint: 200
  }];
  optional string start_time = 2 [(common.field) = {
    default: ""
    examples: "2026-11-08 10:00:00.000"
  }];
  optional uint32 flag = 3 [(common.field) = { enum_number: "boolean" }];
  optional string flag_name = 4 [(common.field) = { enum_str: "boolean" }];
}
```

`default` 和 `examples` 按真实字段的 JSON 类型解释。字符串、bytes、枚举名称、64 位整数、Timestamp/Duration 及相应 wrapper 直接使用文本，不嵌套 JSON 引号。数字和 bool 使用 JSON 文本，例如 `default: "10"`、`default: "false"`。数组和对象也使用 JSON 文本，例如 `default: "[]"`、`examples: "[1,2]"`。examples 可以重复出现；集合的配置作用于整个集合，元素配置放在 `items: {...}`。

`default: "null"` 在字符串字段上就是文本 null；真正的空值使用 `nullable: true default_null: true`，空值示例使用 `nullable: true examples_null: true`。0、false、空字符串、空数组、空对象与 null 分别保留。默认值只供文档和元数据使用，不注入运行时参数。默认值与示例必须通过最终 Schema 校验；大整数使用十进制字符串，比较范围不经过浮点数。

不提供 required 选项。标量/消息可省略时显式使用 optional；repeated/map 沿用可省略语义，oneof 允许不选择并保证互斥。输出模型结合序列化 presence 生成必填项，未设置的消息不会误标必填。

## 枚举

`enum_number` 对照指定枚举的 `field_value`；`enum_str` 对照 `field_name`，大小写精确匹配。二者互斥。支持本地共享枚举名，或完整限定的 Proto enum 名。数字引用只允许整数字段，名称引用只允许 string。默认值、示例、整数范围和枚举成员均在生成时检查；不存在的引用立即失败。

PHP、Node、Rust、Go 在转换 Proto JSON 之前执行生成的枚举校验，支持嵌套消息、集合以及实际使用的 apidoc 分支。省略的 optional 字段不被补值；0 不按缺省值跳过；bool 不当作数字。数字可以使用规范十进制字符串，以兼容 Proto64 和表单参数，不能用 enum 的名称代替数字；enum_str 不接受大小写变体或 field_value。

共享源仍是 PHP 的 `tools/enum_config/**/*.php`。`tools/enum_config/export_proto_catalog.php` 导出 199 个枚举到 `proto/enum_catalog.json`，并同步相邻仓库快照；枚举生成入口已接入该导出。协议生成只读取本地快照，不拉 HTTP/GitHub。

旧的 `ENUM=...` 只是未生效的说明引用，其中包含 -1/0 表示全部等非成员哨兵。本次保留为普通“参考枚举”说明，不擅自新增业务拒绝条件；需要严格限制的字段显式设置 enum_number/enum_str。原 CFG.enum 的实际允许值已迁移为新选项。

## 复用、清除与校验

`common/doc_fields.proto` 用文件级 common.doc_fields 注册唯一名称，例如 `common.model`。description_ref 只共享说明，template_ref 共享整套元数据；实际 Proto 字段名、编号、类型与 presence 不继承。模板可以引用模板，未知、重复、循环及类型不兼容引用会失败。普通 DTO（包括 flow_line_data_item）继续保留。

本地选项覆盖模板，items 递归合并，数组整体替换；显式 false/空字符串有效。clear_default 或 clear:CLEAR_DEFAULT 清除默认值；CLEAR_EXAMPLES、CLEAR_ENUM、CLEAR_FORMAT、CLEAR_PATTERN 分别清除对应配置。不再生成承载说明的 common_field_t。

类型约束为 min_uint/max_uint、min_int/max_int、minimum/maximum（浮点）、min_length/max_length、min_items/max_items、pattern、format、upload。不能覆盖 Proto 的实际类型。数组元素约束必须写在 items；未知选项由编译器报错。

## 接口文档及 HTTP

统一输出并离线验证完整 OpenAPI 3.1，使用官方内置 Schema 和 JSON Schema Draft 2020-12 校验；共享消息进入 components，支持递归引用。目录只取 category_path，每项一层；迁移时已拆分旧斜杠目录。标签、PROJECT:*、语言、责任人、弃用状态/原因均独立输出。认证默认继承项目设置，PUBLIC 不自动取消统一认证。

GET/HEAD 字段使用 query，不生成 body。location 可显式选择 BODY/QUERY/HEADER/COOKIE/PATH，http_name 定义文档参数名；不在接口中重复认证头。path 占位符必须与必填路径参数完全对应。文档参数位置不自动增加服务端绑定，使用前须与实际 HTTP 入口约定一致。无法表达的 GET+apidoc oneof 等组合明确报错。

接口级 examples.request_json 为完整 body，response_json 为完整响应，data_json 为标准包装的 data 区；各自按最终 Schema 校验。字段 examples 采用上述原生文本规则。docs.response_mode 支持 STANDARD_JSON、RAW_JSON、SSE、BINARY、REDIRECT；可指定 response_content_type、message_field 或 response_schema_json。当前 Node GET SSE 已标注 text/event-stream；Rust/Go health 文档使用实际 msg 字段。

TS HTTP 客户端合并 body 与参数类型，按声明方法及 query/path/header/cookie/body 位置发送。内部 RPC 保留既有 POST 约定。PHP/TS RPC JSON 客户端和 TS HTTP JSON 客户端遇到不支持的流式/二进制/重定向响应或媒体类型会报错，禁止静默当作 JSON。已有 multipart HTTP 调用继续支持。

## 统一项目配置

项目根协议配置统一为 `proto/config.json`，五个语言目标采用同一结构；生成器的路径、目标与输出目录仍由 generator.toml 指定。原 PHP 根 config.php 已移除，不执行 PHP 来读取配置。客户端自己的配置文件不属于这一根配置迁移。

```json
{
  "enum_catalog": "enum_catalog.json",
  "new_control_flag": true,
  "project_list": [
    {
      "name": "demo",
      "apifox_project_name": "Demo",
      "apifox_project_id": "123",
      "test_apifox_project_id": "456",
      "apifox_token": "<existing-token>",
      "tag_list": ["demo"],
      "url_replace_cfg": {},
      "export_tags_flag": true,
      "import_options": {
        "endpoint_overwrite": "OVERWRITE_EXISTING",
        "schema_overwrite": "OVERWRITE_EXISTING",
        "update_folder": false
      }
    }
  ]
}
```

import_options 还支持 target_endpoint_folder_id、target_schema_folder_id、target_branch_id、module_id；不填则沿用服务端根目录、主分支和默认模块。覆盖策略可选 OVERWRITE_EXISTING/AUTO_MERGE/KEEP_EXISTING，默认覆盖同 method+path 接口及同名模型，不删除未匹配的资源，不随意改动现有目标值。生成内容由 Proto 管理；需要保留手工修改的项目可显式选 AUTO_MERGE/KEEP_EXISTING。

同步改用官方 `POST https://api.apifox.com/v1/projects/{projectId}/import-openapi`，版本头 2024-03-28。导入前统一校验发布计划；选中目标缺少当前环境 ID、重复最终 method+path、非法替换正则均失败。结果必须包含有效 counters，所有 *Failed 计数为 0，且至少一个接口创建/更新；不确定结果不自动重试。真实发布仍需显式同步命令，本次只执行本地导出/dry-run。

生成、检查、OpenAPI 和 dry-run 不下载工具。首次安装和升级显式执行 tools/proto/prepare.sh，按固定版本及 SHA256 缓存；Linux 容器使用项目目录中的持久缓存。

参考：[官方导入 API](https://apifox-openapi.apifox.cn/api-173409873)、[OpenAPI 3.1 Schema](https://spec.openapis.org/oas/3.1/schema/2022-10-07)、[JSON Schema 校验库](https://docs.rs/jsonschema/0.60.2/jsonschema/)。
