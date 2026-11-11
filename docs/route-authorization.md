# Route authorization (schema version 1)

Authorization is an optional build-time contract, independent of Apifox projects,
tags, documentation titles and rewritten external URLs. Existing projects without
authorization configuration or references retain their previous generated output.
The business `common/api_options.proto` must match the selected generator schema;
upgrade the pinned tool and schema together. Other projects can retain their old
pinned tool and matching schema.

Add an `authorization` root object beside `project_list` in `proto/config.json`:

```json
{
  "authorization": {
    "schema_version": 1,
    "enforced_route_prefixes": ["/llm/"],
    "enforced_source_groups": ["llm"],
    "projects": {"llm": {"tenant_id": 126, "project_type": "llm"}},
    "policies": {
      "provider_llm": {
        "kind": "provider_token",
        "project_keys": ["llm"],
        "allow_account_binding": false,
        "allow_project_sub_token": true,
        "api_types": {"mode": "not_applicable"}
      }
    }
  }
}
```

Use `authorization: { policy: "provider_llm" }` in `option (common.api)`.
The typed field is `ApiOptions.authorization = 18`; `auth: SESSION` is required
for provider policies. Missing references in either enforced route prefixes or
source groups fail compilation. Source groups match a complete directory group
and its descendants, so `llm` excludes `llm_admin`. A custom `http.path` cannot
escape enforcement by its source group. Explicit references outside enforced
scopes are compiled too; undeclared routes outside those scopes retain legacy
behavior. Unknown fields, versions, policies, kinds and projects are rejected.

Project keys and tenant IDs are explicit policy data. Sharing an API requires an
explicit project list; matching `project_type` or duplicate tags grants nothing.
`api_types` also supports `{"mode":"any_of","values":["llm"],"empty_list":"allow"}`
(or `deny`), resolving symbols through the existing `api_type` enum catalog.
The resolved policy contains stable numeric values, not a separate enum source.

All targets emit `route_policy.json` when authorization is configured. Its fields
are `schema_version`, both enforced scope lists, `projects`, `routes` and
`bundle_hash`. Each route contains `route_id` (the source path without `.proto`,
with `/` replaced by `.`), `canonical_path`, `method` (default `POST`) and
`authorization`. Method metadata preserves the existing routing contract; this
feature does not change which HTTP methods a runtime accepts.

Route authorization fields are `schema_version`, `canonical_path`, `policy_id`,
`policy_hash`, `required_capabilities` and `resolved_policy`. The resolved policy
contains `kind`, sorted `project_keys`, their expanded `projects`, both allowance
flags and `api_types`. Capabilities are `provider_token.v1`, plus
`project_sub_token.v1`, `account_binding.v1` or `api_types.v1` when required.
Runtime consumers must reject unsupported versions, capabilities and policies.
Metadata support alone does not implement provider authentication.

Hashes use BLAKE3 over compact recursively key-sorted JSON. `policy_hash` hashes
`{schema_version,resolved_policy,required_capabilities}`. `bundle_hash` hashes the
bundle without its own hash field. Sets are sorted during compilation. Neither
documentation text nor Apifox credentials participate in these hashes.

Runtime registration carries the same authorization:

- TypeScript `cmd_map` entries have an optional typed `authorization` field.
  Generated `authorization_types.ts` supplies types and `route_policy.ts` exports
  the typed bundle for importing without a JSON module loader.
- PHP keeps its existing eight-column route tuple and stores authorization in
  its configuration array. `_uri_no_project` aliases remain outside this work.
- Rust/Go `api.json` entries have an optional `authorization` object.
  Authorized Rust endpoints implement `AUTHORIZATION_JSON: Option<&'static str>`;
  the receiving Endpoint trait supplies a default `None` for legacy endpoints.
  Go registration sets `Endpoint.Authorization` to JSON bytes (compatible with
  `json.RawMessage`). The receiving runtimes must explicitly load or reject it.

Policies, routing and coverage are installed together through the existing
staging/rollback mechanism. Input configuration stays in input fingerprints;
only generated outputs belong in `.proto-gen-manifest.json`. Deployments that
require a project boundary must also independently require its protected scope
and capability at startup, so removing the whole configuration cannot enable
legacy fallback.

Run `cargo test --test authorization`. Set `PROTO_GEN_TEST_TS_NODE_MODULES` and
`PROTO_GEN_TEST_GO_PLUGIN` to exercise actual TypeScript/Go plugin generation in
addition to the always-run PHP/Rust generation and cross-target compiler tests.
