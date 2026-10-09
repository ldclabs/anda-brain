# Anda Brain API 文档（含 TypeScript 类型）

**[English](API.md) | [中文](API_cn.md)**

本文档规定 Rust `anda_brain` 服务的 HTTP 与 MCP 接口（0.13.4，对齐 KIP `11a82ec` /
`kip://profiles/cognitive-memory@2.0.0`）。总览、配置与部署见[技术参考](README.md)；
面向智能体的用法见 [SKILL.md](SKILL.md)；运行时接入与恢复见 [RUNTIME_cn.md](RUNTIME_cn.md)。
Cloudflare Worker 是另一套能力更少的实现，见[其 README](../anda-brain-worker/README.md)
与[宿主契约](../anda-brain-worker/PRODUCT_cn.md)。

## 目录

1. [通用约定](#1-通用约定)
2. [认证](#2-认证)
3. [Memory Interface](#memory-interface)
4. [接口列表](#4-接口列表)——[公共](#41-公共接口)、
   [空间业务](#42-空间业务接口v1space_id)、
   [Wiki](#43-wiki-接口v1space_idwiki)、
   [空间管理](#44-空间管理接口v1space_idmanagement)、
   [管理员](#45-管理员接口admin)、
   [运行时](#authenticated-runtime-inbox-and-observations)
5. [MCP Server](#5-mcp-server)
6. [错误语义](#6-错误语义)
7. [TypeScript 类型定义](#7-typescript-类型定义)
8. [前端调用示例](#8-前端调用示例ts)
9. [Rust 宿主接口](#9-rust-宿主接口)
10. [执行与资源限制](#10-执行与资源限制)

---

## 1) 通用约定

- Base URL：`http://{host}:{port}`（服务默认监听 `127.0.0.1:8042`）。
- 认证头：`Authorization: Bearer <token>`，见[认证](#2-认证)。
- 分片部署：发送与服务端 `SHARDING_IDX` 相同的 `Shard-Id: <index>`（或 `X-Shard`）；默认值为 `0`。
- 支持的序列化格式：
  - 请求：`Content-Type: application/json | application/cbor | text/markdown`
  - 响应：`Accept: application/json | application/cbor | text/markdown`
  - 内容协商仅作用于成功响应体。处理器错误使用 JSON，与 `Accept` 无关；中间件限流（`429`/`503`）和未匹配路由可能返回纯文本或空响应体。
- 大多数接口返回 RPC 包装 `RpcResponse<T>`；`POST /v1/{space_id}/memory` 返回 Memory Interface 的 `Response`，`POST /v1/{space_id}/execute_kip_readonly` 返回 `KipResponse`。
- 字符串时间为 RFC 3339，并规范化为毫秒精度 UTC（`YYYY-MM-DDTHH:mm:ss.SSSZ`）；数值时间为 Unix 毫秒。
- MCP 客户端可使用内置的 Streamable HTTP 端点 `/mcp/<space_id>`，也可使用本地 stdio server：`anda_brain mcp --space-id <space_id> [local|aws]`，见 [MCP Server](#5-mcp-server)。

### 接口遵循的记忆语义

- Formation 或 Maintenance 返回的 conversation id 只用于跟踪一次运行，不是处理回执。要知道记忆何时可用，请使用 Memory Interface 回执（Rust 中可用 `Space::processing_report` / `wait_for_processing`）。
- 记忆强度的衰减在读取时按钉住的强度策略计算（KIP Spec §59.1）。没有任何清扫会写 `memory_strength`，settlement 报告不含衰减字段，`memory_strength_decay_factor` 仍被接受但不起作用。主张到期（`valid_time.until`）同样在读取时判断。
- 宿主对模型写入设闸：引用捕获消息的 Assertion 必须带 `at`（或 `valid.from`），写 `MnemonicState.memory_strength` 必须同时写 `last_metabolized_at` 与宿主绑定的 `strength_policy`（均返回 `ConstraintViolation`）。模型生成的 Formation 请求不得覆盖宿主捕获的 ingest 或 `:msgN` 绑定；学习/runtime Facet 写入返回 `UnsupportedCapability`。原始管理 KIP 仍须满足引擎的完整契约。
- 注意力按 `(raised_seq, ref)` 排序；到期的 Commitment 由 settlement 原生提起（报告字段 `commitments`）。settlement 报告中 Watch 的 `disarmed` 计数也包含 Nexus 的 expired；没有配置语义求值器时，文本 Watch 条件保持 deferred。宿主结算错误通过 `assessment.settlement_errors` 提供给维护模型。
- 未配置可信学习管线时，settlement 的 `skills.unsupported_reason` 说明为何没有运行 Skill 裁决；旧计数保持为零。

---

## 2) 认证

请求通过 `Authorization: Bearer <token>` 携带以下两种凭据之一：

| 凭据 | 是什么 | 来源 |
| --- | --- | --- |
| CWT | Base64 编码的 COSE Sign1 令牌，由 `ED25519_PUBKEYS` 中某个 Ed25519 密钥签名，声明 `sub`（主体）、`aud`（Space id 或 `*`）和 `scope` | 持有受信签名密钥的一方，例如 `anda-cli cwt` |
| Space token | 以 `ST` 开头的不透明值，带 `scope`、可选 `expires_at` 和可选的 Wiki ACL `labels` | `POST /v1/{space_id}/management/add_space_token`；完整值只返回一次 |

**作用域**为 `read`、`write` 和 `*`（`TokenScope`）。`*` 满足任何要求；`read` 和 `write`
只满足要求完全相同作用域的接口，因此 `write` 凭据不能调用私有 Space 的 `read` 接口。
既写入又召回的智能体需要 `*` 凭据，或两种作用域各一份凭据。带标签的 Space token 必须是
`read` 作用域。

接口按以下方式接纳调用方：

| 类别 | 接纳 | 示例 |
| --- | --- | --- |
| 公开读 | public Space 上任何人都可调用。若提供了 Space token，仍会校验以保留其标签限制；结果跨越全部标签的接口拒绝带标签的令牌。私有 Space 需要 `read` 凭据。 | `/recall`、`/recall_structured`、对话记录、Wiki 读取 |
| 宽松公开读 | 同上，但在 public Space 上不校验令牌 | `/info`、`/status`、`/formation_status`、`/memory_status`、`/memory/attention`、`/schema/drafts`、`/probe`、`/execute_kip_readonly` |
| 需凭据 | 具有所需作用域的 CWT 或 Space token | `/formation`、`/maintenance`、`/memory/pin`、`/memory/forget`、Memory Interface 变更类意图 |
| 管理 | 针对本 Space 的 CWT；拒绝 Space token | `/management/*`、`/schema/promote`、`semantic` 遗忘 |
| 管理员 | `sub` 列于 `MANAGERS` 的 CWT | `/admin/*` |
| 运行时 | 经验证的凭据，加上 `BRAIN_RUNTIME_CONFIG` 中的显式映射 | `/attention`、`/attention/{id}/responses`、`/outcomes`、`/runtime/status` |

若 `ED25519_PUBKEYS` 为空，除运行时类别外的所有接口都关闭认证：请求不经签名校验即被接受。
除本地开发外切勿如此部署。运行时接口始终要求经验证的凭据和映射；没有 CWT 验签器时，
独立后果的 HTTP 写入保持关闭。

---

<a id="memory-interface"></a>

## 3) Memory Interface（KIP 2.0，`memory_basic`）

每个 Space 都在 `memory_basic` 级别提供可选的 KIP 2.0 Memory Interface
（`KIP-2.0-Memory-Interface.md`，线上形状见 `kip-memory.schema.json`）：五个意图——
`observe`、`recall`、`revise`、`feedback`、`forget`——共用一种请求形状，一次一个意图。
不声明 `memory_experience`、`memory_learning`、`durable_brain_runtime`、
`receiver_fencing` 与 Capsule 交换；`requires` 中要求它们的请求在执行任何操作前返回
`UnsupportedCapability`。原有端点不变；业务 Agent 应走这条路径。

描述符在 `GET /info`（不含默认 Space）与 `GET /v1/{space_id}/info` 的 `memory_interface`
字段中；原始 KIP 客户端在 `DESCRIBE CAPABILITIES` 与 `DESCRIBE PRIMER` 中看到同一个绑定：

```json
{
  "kip_memory": "2.0",
  "bundles": ["memory_basic"],
  "default_budget": {"max_output_tokens": 4096, "deadline_ms": 30000},
  "tokenizer": "o200k_base@tiktoken-rs-0.12",
  "minimum_response_tokens": 256,
  "default_space": {"id": "my_space"}
}
```

| 端点 | 鉴权 | 请求 / 结果 |
| --- | --- | --- |
| `POST /v1/{space_id}/memory` | recall：`read`（公开 Space 允许匿名）；变更类：`write`；`semantic` forget 需要 CWT（所有者） | Memory Interface `Request` → `Response`（不是 `RpcResponse`） |
| `POST /v1/{space_id}/memory/sources` | `write` | `StageSourceInput` → `RpcResponse<StagedSourceRef>` |
| `GET /v1/{space_id}/memory/sources/{source_ref}` | `read` 凭据 | 暂存该来源的调用方可读 `RpcResponse<StagedSource>` |
| `GET /v1/{space_id}/memory/receipts/{receipt_ref}` | `read` 凭据 | `RpcResponse<{receipt, progress, result, warnings}>` |
| `GET /v1/{space_id}/memory/plans/{plan_ref}` | `read` 凭据 | `RpcResponse<{plan_ref, receipt_ref, plan, host_surfaces}>` |

调用方通过鉴权后，`/memory` 一律返回 HTTP 200，失败放在 Response 里：`status: "failed"`
并带 KIP `error`（`InvalidRequestEnvelope`、`UnsupportedCapability`、`IdempotencyConflict`、
`NotFoundOrNotVisible`、`PreconditionFailed`、`ResultLimitExceeded`、`CursorInvalid`、
`NotAuthorized`、`OutcomeUnknown`、`LegalHoldConflict` 等）。辅助端点使用常规 HTTP 错误映射
（未知或他人的句柄为 404，暂存键冲突为 409）；其 `RpcError.message` 为 KIP 错误信息，
`RpcError.data` 为带 `code` 的 KIP `ErrorObject`。

**句柄归属调用方。** 暂存来源、幂等键、回执、保留的召回依据与擦除计划属于已认证的调用方
（CWT 主体、按名称区分的 Space token，或匿名读者）和该 Space。其他调用方或其他 Space 只会得到
`NotFoundOrNotVisible`，不会得到句柄存在的任何暗示。

```ts
export interface StageSourceInput {
  messages: Message[]; // 1–16 条观察到的消息；消息的 `timestamp`（Unix 毫秒）是它被观察到的时间
  observed_at?: string; // RFC 3339，规范为毫秒 UTC；缺省为捕获时间
  kind?: 'message' | 'tool_trace' | 'artifact';
  order?: { stream_ref: string; event_ref: string; ordinal: number; predecessor_receipts?: string[] };
  idempotency_key: string; // 同键同字节 → 同一句柄；字节不同 → 409
}
export interface StagedSourceRef { source_ref: string; source_digest: string; captured_at: string }

export interface MemoryRequest {
  kip_memory: '2.0';
  request_id?: string;
  operation: 'observe' | 'recall' | 'revise' | 'feedback' | 'forget';
  space?: { id: string }; // 给出时必须是本 Space
  scope?: { task_ref?: string; context_refs?: string[] };
  budget?: { max_output_tokens?: number; deadline_ms?: number; tokenizer?: string };
  idempotency_key?: string; // 变更类必填，recall 不接受
  requires?: ('memory_basic' | 'memory_experience' | 'memory_learning')[];
  input: object; // 各意图见下
}
```

**先暂存再受理。** `observe`、`revise`、`feedback` 引用 `source_ref`：暂存句柄（`src-…`）或
已有的 active Evidence id。准入先于落盘：已被 forget 排除的字节会被拒绝，也不绑定任何键。

**作用域。** `task_ref` 与 `context_refs` 是精确句柄：本 Space 的 active Concept id，或由宿主
映射到作用域 Concept 的不透明字符串（`event_class: "memory_scope"` 的 `Event`，键为
`memory_scope:<handle>`，在第一个命名它的变更中创建）。按键查找句柄时不限生命周期状态（已擦除
除外），所以被 Maintenance 归档、tombstone 或合并的句柄仍指向原作用域。规范上下文集 = 任务 Concept 加每个
上下文的 Concept。来自带作用域来源的每条断言，Formation 都写 `context: :contexts`（闸门拒绝
不写的），Evidence 以及它创建的 Event、Insight、Experience、Commitment 都挂 MemoryScope
Facet；recall 只接纳上下文集包含于请求上下文集的记录（KIP Spec §25.3）。任务名或话题字符串
永远不会选中作用域。

**幂等。** 变更的键作用域为 `(调用方, Space, operation)`；语义包括 operation、请求的作用域、
input 以及来源身份和摘要，不含 `request_id` 与 `budget`。同键同义返回原 `receipt`（附当前进度），
绝不重新抽取；同键异义返回 `IdempotencyConflict`。键、回执与暂存来源在重启后仍在。暂存省略 `observed_at` 时，重试复用首次观察时间。

**进度。** 每个变更返回不可变的 `receipt`（`receipt_ref`、`operation`、`space_id`、`accepted_seq`）
及当前 `progress`：

| 阶段 | 本实现 |
| --- | --- |
| `recorded` | 来源与意图已持久化，Formation 会话在排队、运行，或在一次失败后等待重试（原因见 `progress.reason`）。 |
| `available` | 这一轮已完成；检索索引同步，所以 `available_seq = resolved_seq`。`disposition` 为 `formed`（写入了记忆）、`evidence_only`（只保存了 Evidence）或 `skipped`（什么都没写——这是诚实结果，会在 `warnings` 说明）；完成的 forget 为 `erased`。 |
| `failed` | 终态：来源在处理前已被排除、前驱回执失败、处理被中断且结果未知（`OutcomeUnknown`），或 recording repair 被拒。绝不报告成记忆。 |

变更只有在 available 时才返回 `succeeded`，recorded 时为 `pending`，失败时为 `failed` 并带错误。
变更请求可带 `budget.deadline_ms`，在请求内等待这一轮处理完成。disposition 由宿主根据这一轮
实际提交的内容判定，不由模型决定。`SourceOrder.predecessor_receipts` 必须是调用方自己的回执；
Formation 按 Space 队列顺序处理，前驱失败的后继直接失败，绝不形成。

**observe**（`{source_ref}`）对暂存消息运行 Formation，提示词中带上意图与作用域；结果为
`FormationResult`（`summary`、`memory_refs`——这一轮写入的元素）。

**revise**（`{source_ref, target_ref?, change_kind?}`）写入三种历史之一（KIP Spec §14.2）：

- `correction`：说话者之前的说法错了。Formation 以同一 actor 取代（supersede）它，并保留被更正的区间。
- `world_change`：从变化时刻起的一条新断言；时序继承结束旧值。闸门拒绝这一轮中的任何取代或撤回。
- `misrecorded`：Brain 记下了说话者从未说过的话。带 `target_ref`（错误的断言）时，宿主执行
  **recording repair**（Spec §57.8）：这一轮可以写出原始来源实际表达的主张（引用 `:orig`，
  `asserted_at` 取原始来源时间）；随后宿主在一个受保护事务中把目标标为失效
  （`_system.recording_validity: invalidated`），并登记这些替换断言。不取代、不撤回，说话者的
  生命周期与来源字节不变。没有 `target_ref` 时只把报告保存为 Evidence，响应为 `partial`。
- `unspecified`（默认）：只记录新主张；绝不凭猜测取代，并在 `warnings` 中说明。

**feedback**（`{source_ref, decision_ref?, attempt_ref?}`）由宿主按暂存时的角色保存为 Evidence——
助手的自述是 `agent_statement`，人的反馈是 `user_statement`——绝不是 Outcome，也不评分。
捕获的 Evidence 保留请求作用域（包括引用已有 Evidence 的反馈）。
`decision_ref` / `attempt_ref` 必须是本 Space 的元素。

**forget**（`{target_ref, mode}`）执行 ErasurePlan（Spec §60.7）：

- `payload_only` 清除一条 Evidence 的载荷（`E-…`），或一个暂存来源的字节及从它捕获的 Evidence。
- `semantic`（所有者的决定）经产品删除闭包擦除一条主张（它的元组、其上所有断言、这些断言的
  Evidence 及已记录的依赖者），清除未被引用的 Evidence 或指定元素，抑制来源使 Formation 不再
  重新摄入，并清理宿主副本：暂存字节、Formation 会话记录、引用过被擦除元素的 Recall 会话记录、
  使用账本行与探测缓存。已交付过被擦除元素的召回依据列在 `external_exports` 中。

结果为 `ForgetResult`（`status`、`plan_ref`、`summary`、`coverage_ref`）。只有在 Nexus 依据实际
存储校验了计划、且每个宿主表面都已核实之后，才报告 `completed`（disposition 为 `erased`）；
法律保留为 `blocked`；无法枚举来源的目标为 `partial`。计划可在 `GET …/memory/plans/{plan_ref}`
读取。来源目标还覆盖处理轨迹证明由它新建的 Event、Insight、Experience 和 Commitment；
共享人物及选项概念保留。所有权轨迹缺失或处理未完成时报告 partial。
`POST /memory/forget` 仍是不带计划的技术性元素清除端点。

**recall**（`{query?, target_ref?, mode?, goal?, context?, after?, detail?, time?, attention_cursor?}`）
返回 `Briefing`：

- `after` 中的回执必须属于调用方（否则 `NotFoundOrNotVisible`）；宿主逐个等待到 `deadline_ms`。
  未完成的列在 `coverage.pending_receipts`，响应为 `pending` 且 `action_eligible: false`；失败的
  写进 `uncertainties`。固定的 `time.as_of_seq` 早于某个 `after` 回执的 `available_seq` 时返回
  `PreconditionFailed`。
- `mode: "attention"` 返回作用域内、`attention_cursor` 之后提起的注意力（与
  `GET /memory/attention` 相同的条目）和新游标；不调用模型，不写任何东西。`mode: "resume"`
  在作用域简报上附带这一页（最小 resume：尚未维护 WorkingState）。
- `answer`（默认）与 `action` 针对问题运行一次 Recall；只有当它引用的全部内容都在作用域内时，
  其文字才作为 `summary`。条目由宿主构建：每条被引用的主张都在请求的上下文集、`time.valid_at`
  （`FOR TIME`）与 `time.as_of_seq`（`AS OF SEQ`）下经 `BELIEF` 重新读取，因此 `epistemic_status`
  是最终信念（`insufficient` 绝不等于否定）；原始 Evidence 标为 `source`；被判定误记的主张排除在外。
- 七个通道：`constraints`（精确：作用域内 `insight_class: "constraint"` 的 Insight）与
  `commitments`（精确：pending/blocked 的 Commitment）为必需项，绝不因预算丢弃；`failures`、
  `experiences`、`skills` 为针对问题的有界检索（近似，按其声明的计划完成）；`dependencies`
  检查返回条目自身的 `_system.dependency_validity`；`evidence` 即 Recall 这一轮。过程候选标为
  `standing: "unproven"`（或 `revoked`），不授予任何权限。
- `action_eligible` 要求覆盖完整、屏障满足且没有未核实前提（`action` 模式下 contested 或
  uncertain 的事实即是未核实前提）；它描述记忆是否充分，绝不代表许可。
- `budget.max_output_tokens` 按声明的 tokenizer 限制序列化后的简报（含元数据）。先丢弃可选条目
  （其通道变为 `incomplete`）；必需条目放不下时返回 `ResultLimitExceeded`。其他 tokenizer 返回
  `UnsupportedCapability`。
- 每份简报都保存在 `basis_ref` 之后：ProjectionBasis、每个通道一份 RecallPlan 的 RecallCoverage，
  以及每个条目背后的元素版本。`target_ref`（`basis_ref` 或条目 `ref`）配合 `detail: "evidence"`
  在 `details` 中返回这些内容，按产生该条目的版本读取元素；已变化或已擦除的元素报告为不可用，
  绝不用更新的版本替代。展开有独立的默认预算（65,536 token），且永不 action-eligible。
- recall 不写记忆。返回的元素以 `retrieved` 记入 Nexus 曝光日志（Spec §66.8），它不是认知状态，
  也不强化任何东西。

```ts
export interface MemoryResponse {
  kip_memory: '2.0';
  request_id?: string;
  operation: MemoryRequest['operation'];
  status: 'succeeded' | 'pending' | 'partial' | 'failed';
  receipt?: { receipt_ref: string; operation: string; space_id: string; accepted_seq: number };
  progress?: {
    receipt_ref: string;
    phase: 'recorded' | 'processed' | 'available' | 'failed';
    disposition?: 'formed' | 'evidence_only' | 'skipped' | 'erased';
    resolved_seq?: number; available_seq?: number; reason?: string; error?: KipError;
  };
  result?: unknown; // FormationResult | ForgetResult | Briefing
  error?: KipError;
  warnings: string[];
}
```

MCP 以 `anda_brain_memory`（`{request}`）、`anda_brain_stage_memory_source` 与
`anda_brain_memory_receipt` 暴露同一绑定。

Rust 简报的宿主读取与保留元素固定在同一个快照；期间索引变化会使检索覆盖标为不完整。
摘要检查包含 Evidence 读取与已失效的抽取，但不会把来源记录加入记忆使用计数。

已知限制：`memory_experience` 需要达到 KIP-CognitiveMemory 级的 Nexus（GradingState 与血缘字段的
计算视图、选择依赖尚未实现），因此不声明；`resume` 没有 WorkingState；被关闭打断的 Formation 以
结果未知报告为 `failed`，不会重跑；只有来源字节以内联方式保存的抽取才能做 recording repair；
conformance 适配器的真实模型运行未作为证据记录。

---

## 4) 接口列表

## 4.1 公共接口

### GET `/favicon.ico` 和 GET `/apple-touch-icon.webp`

- 说明：产品图标静态资源
- 鉴权：无
- 响应：`image/x-icon` 或 `image/webp`

### GET `/info`

- 说明：服务信息
- 鉴权：无
- 响应（JSON）：`ServiceInfo`

### GET `/SKILL.md`

- 说明：返回技能描述 Markdown
- 鉴权：无
- 响应：`text/markdown`

---

## 4.2 空间业务接口（`/v1/{space_id}`）

### POST `/v1/{space_id}/formation`

- 作用：提交记忆写入任务
- 鉴权：SpaceToken/CWT `write`
- 请求体：`FormationInput`（Markdown 模式下也允许原始字符串）
- 观察时间接受任意时区偏移、最多毫秒精度的 RFC 3339 时刻，规范为 `YYYY-MM-DDTHH:mm:ss.SSSZ`；无法解析或精度超过毫秒的值直接拒绝（400），不会被替换。缺省时使用已保存的会话创建时间，重试沿用同一回退时间。消息自带的 `timestamp`（Unix 毫秒）是该条消息的观察时间。Formation 按所引用消息的观察时间写入每条主张的 `asserted_at`，而不是 Formation 运行的时间（KIP Spec §13.2）。Markdown 原文按一条 user 消息原样捕获，同样提供 `:msg1` Evidence 绑定。
- 响应（JSON/CBOR）：`RpcResponse<AgentOutput>`
- 响应（Markdown）：`string`（仅返回 `AgentOutput.content`）

### POST `/v1/{space_id}/recall`

- 作用：按自然语言召回记忆
- 鉴权：SpaceToken/CWT `read`（公开空间免鉴权，私有空间需有效 token）。带标签限制的 space token 会收到 `403`，因为 agentic Recall 可跨所有 wiki 标签读取。
- 请求体：`RecallInput`（Markdown 模式下也允许原始字符串）
- 响应（JSON/CBOR）：`RpcResponse<AgentOutput>`
- 响应（Markdown）：纯文本 `AgentOutput.content`

### POST `/v1/{space_id}/recall_structured`

- 作用：返回合成答案、从检索轨迹提取的记忆引用、`found` 和可选不确定性。
- 鉴权和请求体：同 `/recall`；带标签限制的 token 返回 `403`。
- 响应（JSON/CBOR）：`RpcResponse<RecallOutput>`
- 响应（Markdown）：纯文本 `RecallOutput.answer`

<a id="recall-budget-contract"></a>

### Recall 预算合同

可选 `budget` 启用宿主选择的 JSON 记忆包，放在 `content` 中，替代自由生成的答案。
`memory_policy.recall_budget` 可对所有 Recall 强制同一上限；请求只能收紧，不能提高或
关闭策略。策略和请求均省略/null 时保持旧行为。

每次请求先按当前问题执行有界、参数化的概念搜索。只有未完成的承诺（`pending`/`blocked`）
作为必需项；已结束承诺作为可选历史候选。返回的紧凑记录保留 ID、版本和来源，省略重复的
LegacyRecord 原文，并在 `recall_detail` 中列出省略字段。必需属性和原生程序检查不会被
摘要掉。完整 Primer 仍进入规划上下文，交付包仅带紧凑的执行依据；需要详情时可用 KQL
字段投影读取。

可选候选逐条装包；`coverage.partial`、`coverage.omitted` 明示不完整交付。累计模型输入
预算耗尽时，宿主仍可返回已授权读取的候选，同时附上必需警告 `recall_context_budget_exhausted`。
这类响应是局部 `bounded` 包，不是模型合成答案或相关性证明。必需读取不完整，或输出预算
连完整约束和警告都容纳不了时，仍返回 `budget_insufficient`；提供方调用失败也仍按失败处理。

固定 codec 对 compact JSON 记忆包全文计数，含转义和 coverage；`context_tokens`
另行限制本次 Recall 所有规划输入规范序列化的累计 token。提供商消息模板、计费和
RPC/MCP 传输副本不属于这些范围。不根据模型名猜编码，也不回退字符数估算。
预算响应不会附带历史、thoughts、artifacts 或工具调用作为记忆旁路。

`recall_structured` 将同一包放在 `answer`，不从完整 trace 另外复制 citations，并增加
`memory_budget`：`tokenizer`、`token_limit`、`tokens`、`context_token_limit`；此模式的
`found` 仅表示交付了非 Primer 候选，不表示已证明语义相关或完整。Markdown 返回同一
包文本。`budget_insufficient` 或带静态 `failed_reason` 的字面量 `null` 表示不可用/
不充分，不能当成成功的空答案。预算允许时，包内可选的 `failed_reason` 字段携带同一
固定错误代码，用于区分模型调用失败和 token 预算耗尽；该字段计入包的 token 总数，
不会交付提供方的原始错误内容。预算失败使用固定代码：`recall_output_budget_exhausted`、
`recall_required_read_incomplete`、`recall_context_budget_exhausted`、
`recall_deadline_reached`、`recall_model_unavailable`、
`recall_planner_incomplete` 或 `recall_procedure_window_incomplete`。
记忆包始终是候选读取（`semantic_complete=false`、`action_ready=false`）。
必要约束和警告作为整体保留；无法容纳时不交付普通记忆，不用估算或自由答案绕过预算。

### POST `/v1/{space_id}/maintenance`

- 作用：触发维护（睡眠/整理）
- 鉴权：SpaceToken/CWT `write`
- 请求体：`MaintenanceInput`
- 响应：`RpcResponse<AgentOutput>`

`parameters` 中明确提供的值覆盖空间策略；省略项使用空间策略的默认值。同一份有效参数同时用于确定性 settlement 和维护模型，不修改持久化的空间策略。

### POST `/v1/{space_id}/memory/pin`

- 作用：固定或取消固定一个图谱实体（`pinned` 保留类别，不参与保留期归档）。
- 鉴权：SpaceToken/CWT `write`
- 请求体：`MemoryPinInput`；`pinned` 默认 `true`。
- 响应：`RpcResponse<MemoryPinOutput>`

### POST `/v1/{space_id}/memory/forget`

- 作用：物理删除图谱实体；删除前可用 `dry_run: true` 检查影响。
- 鉴权：SpaceToken/CWT `write`
- 请求体：`MemoryForgetInput`；`dry_run` 默认 `false`。
- 响应：`RpcResponse<MemoryForgetReport>`；单个实体的错误位于 `result.entities`。

接受显式的 Concept（`C-*`）、Proposition（`P-*`）、Assertion（`A-*`）、Evidence（`E-*`）和 Activity（`X-*`）ID，包括保存消息原文的 Evidence。原生 legal hold 和引用检查仍然生效，成功清除后保留已擦除身份桩。计数分别报告各类被清除记录，包含级联删除。此操作清除所选图谱记录；已保存的会话、wiki 文档及外部副本各有独立生命周期。
引用已清除元素的产品预览会在报告成功前清理；预览清理失败会写入该实体的错误字段。

### GET `/v1/{space_id}/memory/attention`

- 作用：注意力召回（KIP Memory Interface §4）：返回调用方所保存游标之后，已触发的 Watch 与 settlement 判定到期的 Commitment。
- 鉴权：SpaceToken/CWT `read`；公开空间允许匿名读取。
- 查询参数：`AttentionRecallInput`——`attention_cursor`（可选）、`limit`（1–50 个条目，默认 20）。
- 响应：`RpcResponse<AttentionRecall>`，保持 JSON/CBOR/Markdown 协商。游标或 limit 无效返回 400。

```ts
export interface AttentionRecallInput {
  attention_cursor?: string; // 不透明；缺省、"attention:start" 或旧的 "attention:-1" 表示从第一次提起开始
  limit?: number; // 1–50 个条目；默认 20
}

export interface AttentionRecall {
  items: AttentionItem[]; // 按 (raised_seq, ref) 排序
  // 最后交付的位置：页面停在一次提交中间时为 "attention:<seq>:<ref>"；该次提交及之前的
  // 条目全部交付时为 "attention:<seq>"；没有新条目时为输入游标（或 "attention:start"）
  attention_cursor: string;
  complete: boolean; // 输入游标之后的所有提起是否都已读完
}

export interface AttentionItem {
  ref: string; // 被提起的 Watch 或 Commitment
  kind: 'watch_fired' | 'commitment_due';
  summary: string;
  raised_seq: number; // watch_fire / commitment_review Activity 的 space_seq
  due_at?: string;
  target_refs: string[]; // Watch 的 `watches` 目标，或该 Commitment
  priority?: number;
}
```

每个条目都由一次提交提起：`watch_fire` Activity，或指名某个到期 Commitment 的 `commitment_review` Activity。settlement 为每个到期、状态为 `pending`/`blocked` 且没有 Watch 的 Commitment 原生写一条这样的 Activity，键为 `commitment_review:<commitment id>:<due_at>`（Profile §17）：重放不会再次提起，只有新的 `due_at` 才会再次提起；Commitment 的状态不变。一次提交可以提起多个条目，页面可以停在它们中间，下一页从最后交付的条目之后继续。读取不改变记忆，游标不会过期，调用方取走条目后自行保存。条目不授予任何权限：据此行动仍须经过 action gate 与 Governance。它与有身份的运行时收件箱（`GET /v1/{space_id}/attention`，按 action gate 的 wake 记录分页）相互独立。

### GET `/v1/{space_id}/schema/drafts`

- 作用：本空间的草稿词汇（KIP §20.16）：Formation 用 `DEFINE`（或已弃用的 `declare_memory_symbols`）起草的每个符号、其定义，以及晋升后所并入的谱系。
- 鉴权：SpaceToken/CWT `read`；公开空间允许匿名读取。
- 响应：`RpcResponse<SchemaDrafts>`。

```ts
export interface SchemaDrafts {
  package_ref: 'kip://local/draft@0.0.0';
  schema_environment_version: number;
  symbols: DraftSymbol[];
}

export interface DraftSymbol {
  kind: 'ConceptType' | 'PredicateType';
  name: string;
  ref: string; // kip://local/draft@0.0.0/<name>；元素永久保留这个引用
  definition: object; // 起草时的定义
  promoted_to?: string; // 晋升目标谱系，如 kip://profiles/cognitive-memory/Person
}
```

每个新草稿排一个 `review_schema` SleepTask，键为 `review_schema:<kind>:<ref>`（带 `symbol_kind` / `symbol_ref` 属性）。Maintenance 审阅它，可以在总结里提议晋升，但从不定义或晋升符号。Formation 定义符号的请求只能包含 `DEFINE`（最多 8 个），名称与定义体必须是字面量并符合本部署的命名形状；每个空间自有符号最多 512 个。

### POST `/v1/{space_id}/schema/promote`

- 作用：把一个草稿符号晋升到已安装包中同类别的符号（KIP §20.16）。这是 Schema 迁移：草稿下写入的元素保留原有的 `schema_ref` / `predicate_ref`，从新的 Schema Environment 版本起，类型与谓词匹配把两条谱系视为一条。每个草稿至多晋升一次，从不隐式晋升。
- 鉴权：管理 CWT `write`（代表 `manage_schema`）；Space token 不能晋升。
- 请求体：`{kind: 'ConceptType' | 'PredicateType', from: string, to: string}`——`from` 是草稿的本地名或确切引用，`to` 是已安装符号的确切引用或无歧义的本地名。
- 响应：`RpcResponse<{promoted: string, to: string, schema_environment_version: number}>`。草稿不存在、类别不符、目标无法解析或重复晋升均返回 400。

### GET `/v1/{space_id}/memory_status`

- 用途：读取记忆统计和最近一次维护报告。
- 鉴权：SpaceToken/CWT `read`；公开空间允许匿名读取。
- 响应：`RpcResponse<MemoryStatus>`，保持 JSON/CBOR/Markdown 协商。
- `result.last_settlement.correction_scan_incomplete`：本次有界扫描尚未证明 backlog 已耗尽，后续维护会从事务内部继续。
- `result.last_settlement.correction_scan_through_seq`：已完整读取的最大事务序号。
- `result.last_settlement.correction_scan_error`：扫描失败原因；失败不会推进游标。
- 这些字段表示更正发现进度，不表示模型完成审查或 Watch 获得完整授权覆盖。

### POST `/v1/{space_id}/execute_kip_readonly`

- 作用：执行 KIP 请求（只读模式，适用于查询）
- 鉴权：SpaceToken/CWT `read`（公开空间免鉴权，私有空间需有效 token）
- 请求体：`KipRequest`，或直接一个 JSON 字符串（按单条命令解析）
- 若 `operations` 超过一项，必须提供 `execution.mode`；即使 HTTP 返回 `200`，也要检查顶层 `status` 和每个 `results[].status`。
- 响应：`KipResponse<T>`（结果类型随命令而定）
- 只读由命令**解析出的语义**决定：无论请求怎么标注，KML 变更都会在这里被拒绝

### POST `/v1/{space_id}/get_or_init_user`

- 作用：按给定 principal 获取或初始化用户 Concept 节点
- 鉴权：SpaceToken/CWT `write`
- 请求体：`GetOrInitUserInput`
- 省略 `name` 时保留已有显示名；显式提供 `name` 时更新显示名。首次创建且未提供姓名时，使用 key 作为初始显示名。
- 响应：`RpcResponse<Concept>`

### GET `/v1/{space_id}/info`

- 作用：获取空间状态和统计
- 鉴权：SpaceToken/CWT `read`（公开空间免鉴权，私有空间需有效 token）
- 响应：`RpcResponse<SpaceInfo>`

### GET `/v1/{space_id}/status`

- `/v1/{space_id}/info` 的别名，鉴权和响应相同。

### POST `/v1/{space_id}/probe`

- 作用：无需模型的检索可达性检查；`found:false` 不代表信念被否定。
- 鉴权：沿用 Space 的 `read` 权限和公开空间读取规则。
- 请求：`{"query":"...", "limit":8}`。
- 响应：`RpcResponse<ProbeOutput>`，含 `found`、`negative_cached`、可选 `hits` 和可选 `search_exhaustive`。后者来自 SEARCH result 顶层覆盖字段；缺省表示未知，有剩余分页时为 false。负缓存仅保存明确穷尽的检索 miss，不能作为不存在或信念被否定的证明。

### GET `/v1/{space_id}/formation_status`

- 作用：获取记忆写入状态（更轻量级的接口，专门用于监控记忆写入进度）
- 鉴权：SpaceToken/CWT `read`（公开空间免鉴权，私有空间需有效 token）
- 响应：`RpcResponse<FormationStatus>`
- 此接口是轻量监控，游标不是逐任务成功证明。Rust 宿主可用 `Space::processing_report` / `wait_for_processing` 区分排队、运行、失败、取消、中断和超时；超时后继续核对同一 conversation ID，不重新提交。

### GET `/v1/{space_id}/conversations/{conversation_id}?collection=<collection>`

- 作用：获取单条会话详情
- 鉴权：SpaceToken/CWT `read`（公开空间免鉴权，私有空间需有效 token）；带 ACL 标签限制的 token 返回 `403`——会话持久化了完整的 agent 运行历史，不受标签过滤；`collection=recall` 对公开空间的匿名访问也返回 `403`（私有期的 recall 运行可能内嵌 labeled wiki 内容）
- Query:
  - `collection?: string` // "formation"（默认）、"recall" 或 "maintenance"；未知值返回 `400`
- 响应：`RpcResponse<Conversation>`

### GET `/v1/{space_id}/conversations/{conversation_id}/delta?collection=<collection>&messages_offset=<n>&artifacts_offset=<n>`

- 作用：按客户端已消费的 offset 获取会话增量更新
- 鉴权：SpaceToken/CWT `read`（公开空间免鉴权，私有空间需有效 token）；带 ACL 标签限制的 token 返回 `403`（`collection=recall`：公开空间匿名访问同样拒绝）
- Query:
  - `collection?: string` // "formation"（默认）、"recall" 或 "maintenance"；未知值返回 `400`
  - `messages_offset?: number` // 仅返回该偏移量之后的新消息，默认 `0`
  - `artifacts_offset?: number` // 仅返回该偏移量之后的新 artifacts，默认 `0`
- 响应：`RpcResponse<ConversationDelta>`

### GET `/v1/{space_id}/conversations?collection=<collection>&cursor=<cursor>&limit=<n>`

- 作用：分页列出会话
- 鉴权：SpaceToken/CWT `read`（公开空间免鉴权，私有空间需有效 token）；带 ACL 标签限制的 token 返回 `403`（`collection=recall`：公开空间匿名访问同样拒绝）
- Query:
  - `collection?: string` // "formation"（默认）、"recall" 或 "maintenance"；未知值返回 `400`
  - `cursor?: string`
  - `limit?: number`
- 响应：`RpcResponse<Conversation[]>`（并通过 `next_cursor` 给出下一页游标）

---

## 4.3 Wiki 接口（`/v1/{space_id}/wiki`）

Wiki 是空间的版本化参考记忆（政策、手册、SOP、API 文档）。写入是 Git 式不可变提交（CAS 并发控制）；检索返回可校验的 `wiki://` 引用。ACL：文档可携带 `acl_label`；带 `labels` 的 space token 仅可见无标签内容 + 所授标签——查询预过滤后仍核对文档当前版本、状态和权限。权限检查与版本选择共享同一文档快照。公开空间的匿名读者仅可见无标签内容；越权一律表现为 404。

Wiki 专属错误语义：`409` 提交冲突（`RpcError.data.current_version` 为应 rebase 的版本）、`413` 内容超 1 MiB、`404` 不存在或 ACL 拒绝。

目录由 Markdown 的 ATX 标题（`#`–`######`）生成，独立于检索分块；读取 section 包含该标题的子章节。锚点按标题派生，同名标题加序号。`full`、`range` 和 `section` 均最多返回 256 KiB；`truncated` 为 true 时，使用返回的 `byte_range` 继续读取。历史和引用校验只接受已发布 parent 链中的版本，失败提交的残留不是历史。

OKF 导入按完整文件替换 title、tags、resource、type 和未知 frontmatter 键值，删除字段会清空对应导入状态；已有 ACL 与其他宿主 metadata 保留。标题或标签经 commit 修改后，导出仍保留未知键值。YAML 使用标准解析和序列化，不承诺注释或原始排版保真。

WikiDigest 默认关闭。开启后，commit、archive、restore 和 ACL 变化持久化文档待处理标记；每轮最多处理 20 个文档，启动、维护后或显式调用时推进，失败文档保留待重试。正文校验和未变时复用已有账本，不再调用模型。提取漏项不是撤回依据：对旧断言必须在每个正文批次中明确核验为 `absent`，缺失、重复或 `unknown` 判断均保留原断言。归档或加标签由后续 digest 撤回本来源断言；该图谱同步是异步的。模型处理中发生文档变更时，不发布旧结果，保留新一代待处理标记。观测高水位 `wiki_digested` 不表示队列已经清空。


### POST `/v1/{space_id}/wiki/docs`

- 作用：提交文档（创建；或携带 `doc_id` + `parent_version` 做 CAS 更新）；同内容提交为零写入
- 鉴权：SpaceToken/CWT `write`
- 请求体：`WikiCommitInput`（也接受原始 Markdown 字符串，标题取首个标题行）
- 响应：`RpcResponse<WikiCommitOutput>`

### GET `/v1/{space_id}/wiki/docs?namespace=<ns>&status=<status>&tag=<tag>&cursor=<cursor>&limit=<n>`

- 作用：分页列出文档
- 鉴权：SpaceToken/CWT `read`（公开空间免鉴权；ACL 标签生效）
- 响应：`RpcResponse<WikiDocInfo[]>`（下一页游标经 `next_cursor` 返回）

### GET `/v1/{space_id}/wiki/docs/{doc_id}`

- 作用：文档元信息 + 目录（TOC）
- 鉴权：SpaceToken/CWT `read`（公开空间免鉴权；ACL 标签生效）
- 响应：`RpcResponse<{ doc: WikiDocInfo; toc: WikiTocEntry[] }>`

### GET `/v1/{space_id}/wiki/docs/{doc_id}/content?version=<id>&anchor=<anchor>&start=<n>&end=<n>`

- 作用：渐进读取——`anchor` 读单节、`start`+`end` 读字节区间、均不传读受限全文；`version` 读历史版本
- 鉴权：SpaceToken/CWT `read`（公开空间免鉴权；ACL 标签生效）
- 响应：`RpcResponse<WikiReadOutput>`

### GET `/v1/{space_id}/wiki/docs/{doc_id}/versions?cursor=<cursor>&limit=<n>`

- 作用：版本历史（不可变提交链）
- 鉴权：SpaceToken/CWT `read`（公开空间免鉴权；ACL 标签生效）
- 响应：`RpcResponse<WikiVersionInfo[]>`（下一页游标经 `next_cursor` 返回）

### POST `/v1/{space_id}/wiki/docs/{doc_id}/archive`

- 作用：归档文档（退出检索，仍可按 id 读取，可恢复）
- 鉴权：SpaceToken/CWT `write`
- 响应：`RpcResponse<WikiDocInfo>`

### POST `/v1/{space_id}/wiki/docs/{doc_id}/restore`

- 作用：恢复归档文档进入检索
- 鉴权：SpaceToken/CWT `write`
- 响应：`RpcResponse<WikiDocInfo>`

### POST `/v1/{space_id}/wiki/search`

- 作用：BM25 关键词检索，返回片段与可校验引用
- 鉴权：SpaceToken/CWT `read`（公开空间免鉴权；ACL 标签生效）
- 请求体：`WikiSearchInput`（也接受原始查询字符串）
- 响应：`RpcResponse<WikiSearchOutput>`

### POST `/v1/{space_id}/wiki/verify`

- 作用：对照不可变存储校验引用
- 鉴权：SpaceToken/CWT `read`（公开空间免鉴权；ACL 标签生效）
- 请求体：`WikiVerifyInput`（也接受原始 `wiki://` URI 字符串）
- 响应：`RpcResponse<WikiVerifyOutput>`

### GET `/v1/{space_id}/wiki/events?kind=<kind>&doc_id=<id>&cursor=<cursor>&limit=<n>`

- 作用：查询 append-only 审计日志（写入、导入、蒸馏；开启 `wiki_audit_reads` 后含读操作）
- 鉴权：SpaceToken/CWT `read`；受 ACL 标签限制的 token 返回 `403`
- 响应：`RpcResponse<WikiEventInfo[]>`（下一页游标经 `next_cursor` 返回）

### POST `/v1/{space_id}/wiki/import`

- 作用：导入 OKF v0.1 bundle；checksum 幂等（重复导入零版本膨胀）；未知 frontmatter 键值按结构保留；YAML 注释、字段顺序及标量排版不保留
- 鉴权：SpaceToken/CWT `*`（全量 scope）
- 请求体：`WikiImportInput`
- 响应：`RpcResponse<WikiImportOutput>`

### GET `/v1/{space_id}/wiki/export?namespace=<ns>`

- 作用：按 namespace 导出 OKF bundle（concept `.md` + `index.md` + 含校验和的 `manifest.json`）；可在空库完整重放
- 鉴权：SpaceToken/CWT `*`（全量 scope）
- 响应：`RpcResponse<WikiExportOutput>`

### POST `/v1/{space_id}/wiki/digest`

- 作用：把待处理 wiki 版本蒸馏进 Cognitive Nexus（每条事实写成 Proposition + 归属于 Brain 的 Assertion，并引用对应段落作为 Evidence）；经完整正文批次明确核验不再支持的旧断言，digest 撤回自己的 Assertion；漏提取或未知判断不会触发撤回，Proposition 与他人的 Assertion 不受影响（需先 `update_space {"wiki_digest": true}` 开启）
- 鉴权：SpaceToken/CWT `write`
- 响应：`RpcResponse<WikiDigestReport>`

---

## 4.4 空间管理接口（`/v1/{space_id}/management`）

### GET `/v1/{space_id}/management/space_tokens`

- 作用：列出 Space Token
- 鉴权：必须通过 CWT `write`（用户管理级鉴权）
- 响应：`RpcResponse<SpaceToken[]>` —— `token` 字段仅显示前缀（如 `STabc123…`）；完整 token 值只在 `add_space_token` 响应中出现一次，铸造时务必保存，或后续凭 `name` 吊销

### POST `/v1/{space_id}/management/add_space_token`

- 作用：新增 Space Token
- 鉴权：必须通过 CWT `write`（用户管理级鉴权）。铸造 `*`（全 scope）token 需要 `*` scope 的 CWT——`write` CWT 不能铸造高于自身 scope 的 token
- 请求体：`AddSpaceTokenInput` —— `name` 必填且空间内唯一。仅 `read` token 可设置 `labels`；`[]` 表示仅可读无标签 wiki 内容，省略表示不受标签限制。
- 响应：`RpcResponse<SpaceToken>`（新 token，前缀总是 `ST`；这是唯一携带完整 token 值的响应）

### POST `/v1/{space_id}/management/revoke_space_token`

- 作用：吊销 Space Token
- 鉴权：必须通过 CWT `write`（用户管理级鉴权）
- 请求体：`RevokeSpaceTokenInput` —— 传 `token`（完整 token 值）或 `name`（唯一 token 名称，供未保存 token 值的管理者使用）
- 响应：`RpcResponse<boolean>`（是否成功吊销）

### PATCH `/v1/{space_id}/management/update_space`

- 作用：更新空间信息、wiki 设置及可选的 `memory_policy`（校验后作为新策略持久化）。
- 鉴权：必须通过 CWT `write`（用户管理级鉴权）
- 请求体：`UpdateSpaceInput`
- 响应：`RpcResponse<true>`

### POST `/v1/{space_id}/management/shadow_eval`

- 作用：在空间副本上重放近期 Recall，对比候选记忆策略和当前策略；可能产生多次模型调用。
- 鉴权：CWT `write`（不接受 space token）。
- 请求体：`ShadowEvalInput`；`replay_sample` 默认采用空间策略，上限为 `16`。
- 响应：`RpcResponse<ShadowReport>`

### PATCH `/v1/{space_id}/management/restart_formation`

- 作用：通过会话 ID 重启记忆写入任务（用于失败/过期的写入任务）
- 鉴权：必须通过 CWT `write`（用户管理级鉴权）
- 请求体：`FormationRestartInput`
- 响应：`RpcResponse<true>`

### GET `/v1/{space_id}/management/space_byok`

- 作用：获取 BYOK（Bring Your Own Key）配置，即使用自定义模型配置
- 鉴权：必须通过 CWT `write`（用户管理级鉴权；响应包含模型供应商凭据）
- 响应：`RpcResponse<ModelConfig>`

### PATCH `/v1/{space_id}/management/space_byok`

- 作用：更新 BYOK（Bring Your Own Key）配置，即使用自定义模型配置
- 鉴权：必须通过 CWT `write`（用户管理级鉴权）
- 请求体：`ModelConfig`
- 响应：`RpcResponse<true>`

---

## 4.5 管理员接口（`/admin`）

### POST `/admin/create_space`

- 作用：创建空间
- 鉴权：平台管理员 + CWT `write`
- 请求体：`CreateOrUpdateSpaceInput`
- 响应：`RpcResponse<SpaceInfo>`

### POST `/admin/{space_id}/update_space_tier`

- 作用：更新空间 tier
- 鉴权：平台管理员 + CWT `write`
- 请求体：`CreateOrUpdateSpaceInput`
- 响应：`RpcResponse<SpaceTier>`

---

<a id="authenticated-runtime-inbox-and-observations"></a>

## 4.6 运行时接口（有身份的待办与后果）

启动前配置 `BRAIN_RUNTIME_CONFIG`。完整合同、恢复及示例见 [RUNTIME_cn.md](RUNTIME_cn.md)
和 [runtime.example.json](runtime.example.json)。公共/本地 Space 也必须提供真实凭据。

| 接口 | 授权 | 返回 |
| --- | --- | --- |
| `GET /v1/{space_id}/attention?limit=20&cursor=...` | 真实 read/* 凭据、原生主体映射和 audience | `AttentionPage`，读取不领取 |
| `POST /v1/{space_id}/attention/{id}/responses` | 真实 write/*、当前可见性及正确接收者 | `ResponseReceipt`，body 为 `AttentionResponse` |
| `POST /v1/{space_id}/outcomes` | 验签的观察者 CWT、当前 record_outcome 及登记合同 | `ObservationReceipt`，body 为 `OutcomeInput` |
| `GET /v1/{space_id}/runtime/status` | 真实 read/*，配置启用时需映射 | `RuntimeStatus`，计数仅覆盖可见的有限页面 |

URL 中的 id 是返回的 wake 摘要，不是带斜杠的 wake_ref。五分钟有效的游标经加密认证，
绑定调用者/instance/配置；跨调用者或过期游标被拒绝。公共 Space 和普通 write token
不能认证独立观察者。MCP 暴露 get_attention、respond_attention、get_runtime_status
（均带 anda_brain_ 前缀），观察者写入不属于模型工具。

POST 使用结构化 JSON/CBOR，响应保留 JSON/CBOR/Markdown 协商。同事件同文幂等，
异文 409 并留存审计；未来时间或错误 instance 被拒绝。迟到/更正/安全状态、原生提交
和 learning 入样分别报告。learning 变体必须符合原有 OutcomeMeasurements 合同，
包括 baseline 在内均只路由到既有控制器。

```ts
type RuntimeScope = { space_id: string; space_instance: string };
type AttentionQuery = { cursor?: string | null; limit?: number | null };
type AttentionResponse =
  | { kind: "clarification"; event_key: string; answer: string }
  | { kind: "agent_statement"; event_key: string; statement: string };
type ResponseReceipt = {
  receipt_id: string; status: string; evidence_ref: string | null;
};
type AttentionPage = {
  scope: RuntimeScope; items: AttentionItem[];
  next_cursor: string | null; complete: boolean;
};
type AttentionItem = {
  id: string; wake_ref: string; parent_id: string | null;
  watch_ref: string; fire_activity_ref: string; summary: string;
  state: "pending" | "running" | "blocked" | "completed" | "cancelled";
  reason: string | null; decision: Record<string, unknown> | null;
  decision_ref: string | null; attempt_ref: string | null;
  dispatch_ref: string | null; clarification: Record<string, unknown> | null;
  delivery: Record<string, unknown> | null;
};
type OutcomeStatus = "success" | "partial" | "failure" | "aborted" | "unknown";
type OutcomeInput = {
  space_instance: string; attempt_ref: string;
  observer_configuration_digest: string; event_key: string;
  observed_at: string; metric: string; window: string;
  observation:
    | { kind: "measurement"; terminal: boolean; outcome_status: OutcomeStatus;
        magnitude?: number | null; payload: unknown }
    | { kind: "learning"; measurements: Record<string, unknown> };
  correction_of?: string | null; safety_signal?: string | null;
  utility?: { witness_ref?: string; witness?: ContributionWitness } | null;
};
type ObservationReceipt = {
  format: "anda-brain:observation-receipt-v1";
  receipt_id: string; scope: RuntimeScope; event_key: string;
  body_digest: string; observer: string; received_at_ms: number;
  observed_at: string; status: string;
  native_committed: boolean; learning_eligible: boolean;
  outcome_status: OutcomeStatus | null; outcome_ref: string | null;
  observation_ref: string | null; reason: string | null; safety_pending: boolean;
  safety_evaluation_ref?: string; // 解决或覆盖本安全信号的原生撤销记录。
};
type RuntimeStatus = {
  supported: boolean; configured: boolean; scope: RuntimeScope | null;
  attention_enabled: boolean; actions_enabled: boolean;
  observation_enabled: boolean; observer_authenticated: boolean;
  blocked_reasons: string[]; visible_items: number; inventory_complete: boolean;
  utility?: UtilityStatus;
  learning?: LearningRuntimeStatus;
  semantic_attention?: SemanticAttentionStatus;
  trust?: TrustRuntimeStatus;
};
```

### 后果与回答语义

待办页包含 1–50 项、最多 256 KiB 可见输出。`complete` 仅表示本次快照遍历结束，
不表示任务完成或语义完整；每页重验可见性，包括 gate 记忆包背后的原生证据。
MCP 回答参数为 `{id, response}`，列表参数为 `{cursor, limit}`。读取和回答均不领取
任务；澄清回答须对应已提交 ask、正确接收者和有效期限，回答进入新 gate，不授予权限。
`agent_statement` 只生成带归属 Evidence 和 Activity，不生成独立 OutcomeRecord。例如：

```json
{"kind":"clarification","event_key":"answer-42","answer":"明天"}
```

测量请求使用实际保留的身份与摘要：

```json
{
  "space_instance":"sha256:INSTANCE_DIGEST",
  "attempt_ref":"X-42",
  "observer_configuration_digest":"sha256:REGISTERED_METHOD_DIGEST",
  "event_key":"instrument-event-42",
  "observed_at":"2026-09-17T10:00:00.000Z",
  "metric":"delivery",
  "window":"durable_inbox_v1",
  "observation":{
    "kind":"measurement",
    "terminal":true,
    "outcome_status":"success",
    "magnitude":null,
    "payload":{"delivery_digest":"sha256:ACTUAL_DELIVERY_REQUEST_DIGEST"}
  },
  "correction_of":null,
  "safety_signal":null
}
```

观察合同固定主体、方法/配置摘要、控制域、任务族、指标、窗口及允许延迟。接收层验证
真实 act/Attempt、原生事务作者、已存在分派和观测时间，拒绝控制器自评。inbox 成功还
须验证实际持久化投递及匹配的摘要/时间，仅证明 inbox 写入，不代表人已阅读或业务成功。
其他回调须提供自己的独立测量合同。

普通合格测量原子生成 Outcome Evidence 与 `outcome_observation` Activity；原始材料
保留在 Evidence payload，不扩展封闭的 OutcomeRecord Facet。非终结/unknown 不完成
分派，缺失 magnitude/cost 不填零。终结证据对账同一 Attempt，取消后也可保留真实后果。
原生留证与分派对账是两个可恢复步骤；未确认写入须用相同事件/内容和新鲜认证重试。
调用方取消不截断已接收写入。

认证后的迟到、冲突及学习非终结材料可审计，不改变旧试验入样资格。更正指向同一
观察者/Attempt 的较早事件，追加证据，不改写旧 Outcome/Evaluation。迟到或排除的
安全信号仍可发现；显式启用的学习消费者使用当前观察者权限，在原生撤销完成后才确认
`safety_pending` 并记录 `safety_evaluation_ref`。

已登记学习 Attempt 使用 `observation:{"kind":"learning", "measurements":{...}}`。
归属来自保留的 ticket，`trial_ref:null` 的 baseline 也不会落入普通写入器；只有
`LearningRuntime::submit_outcome` 写原生结果，保持冻结 cutoff、可比性和 ACK 恢复。
控制器不可用时拒绝，不转普通路径。`learning_eligible:true` 只表示合同接纳，不等于
正向裁决。输入上限 16 KiB，事件键 256 字节；身份装配和存储上限见[运行时指南](RUNTIME_cn.md)。

### 学习运行状态

既有 status 路由和 MCP 工具新增 learning；模型工具不提供 cohort 报名或观察者
凭据管理。只有明确映射的 auditor 能看到 capacity / last_pass，其他调用方为 null。
Maintenance 的可选 skills.runtime 显示独立调度状态，旧 Skill 计数不会重复计入
此前调度工作。详见[学习指南](LEARNING_RUNTIME_cn.md)。

```typescript
type LearningRuntimeStatus = {
  compiled: boolean; registered: boolean; registration_enabled?: boolean;
  bindings_ready: boolean; automatic_allowed: boolean; running?: boolean;
  automation?: { trials: boolean; reviews: boolean; archive: boolean; safety: boolean };
  blocked_reasons?: string[];
  capacity?: { hot_jobs: number; maximum_hot_jobs: number; archived_jobs: number;
    retained_identities: number; reserved_bytes: number;
    storage: { maximum_records: number; maximum_reserved_bytes: number } } | null;
  last_pass?: { started_at_ms: number; finished_at_ms: number; enrolled: number;
    driven: number; observed: number; settled: number; archived: number;
    reviews_checked: number; reviews_enrolled: number; safety_resolved: number;
    blocked: string[] } | null;
};
```

### 交付与效用元数据

结构化 Recall 新增可选 recall_receipt 元数据，answer/语义 packet 保持不变。普通或
直接 agent 调用可通过 conversation ID 在可信 Rust API 查询收据。观察者在 outcomes
的 utility 中可选择内联见证或既有原生见证引用，不放宽验签与独立权限检查。
运行状态新增 utility 的开关、校准状态及恢复原因；模型没有设置分数或新增 MCP
变更工具。详见 [UTILITY_RUNTIME_cn.md](UTILITY_RUNTIME_cn.md)。

```typescript
type RecallReceiptRef = {
  id: string; digest: string; scope: RuntimeScope;
};
// RecallOutput.recall_receipt?: RecallReceiptRef
// OutcomeInput.utility?: { witness_ref?: string; witness?: ContributionWitness }
// RuntimeStatus.utility?: UtilityStatus
type UtilityStatus = {
  configured: boolean; automatic: boolean; apply: boolean;
  calibrated: boolean; ranking: boolean; running: boolean;
  reason: string | null;
};
```

### 文本注意力状态

Runtime status 新增可选 `semantic_attention`：`configured`、`automatic`、`running`、
`pin`、`reason` 及可空的 `last_pass`。后者包含单轮有界统计 `scanned`、`calls`、
`input_tokens`、`advanced`、`fired`、`expired`、`deferred`、`reason`，仅配置为 auditor
的调用者可读取，其他调用者得到 null。它不是完整清单，也不证明所有文本 Watch
均已求值。既有 JSON/CBOR/Markdown 和 HTTP/MCP 请求形状保持兼容；不新增模型配置
或求值写接口。操作员通过每 Space 的 `semantic` 启动绑定安装。预算、pin 迁移、
unknown/deferred、原生 Artifact 擦除及幂等恢复见
[SEMANTIC_WATCH_RUNTIME_cn.md](SEMANTIC_WATCH_RUNTIME_cn.md)。

```typescript
type SemanticAttentionStatus = {
  configured: boolean; automatic: boolean; running: boolean;
  pin: { id: string; digest: string } | null; reason: string | null;
  last_pass: {
    scanned: number; calls: number; input_tokens: number; advanced: number;
    fired: number; expired: number; deferred: number; reason: string | null;
  } | null;
};
```

### 上下文 trust 状态与发现提示

既有 HTTP 状态路由和 MCP 状态工具新增可选 `trust` 元数据。`governor_authorized`
表示配置的治理主体当前是否具有原生权限，不是授予调用者的权限。不新增 HTTP/MCP
trust 管理或独立事实写工具，既有 JSON/CBOR/Markdown 请求保持兼容。正常认证的
独立 measurement payload 可选携带 `trust_verification_ref: "E-…"`，用于公告既有原生
事实核验记录并有界发现，不能把行动成功/失败转成信任得分。可信 Rust 事实接收、
审查、应用、版本冲突恢复和限定域回退见 [TRUST_RUNTIME_cn.md](TRUST_RUNTIME_cn.md)。

```typescript
type TrustRuntimeStatus = {
  configured: boolean; automatic: boolean; apply: boolean; automatic_apply: boolean;
  calibrated: boolean; governor_authorized: boolean; running: boolean;
  reason: string | null;
};
```

---

## 5) MCP Server

默认情况下，HTTP 服务会暴露支持流式传输的 HTTP MCP 端点；`MCP_HTTP_ENABLED=false` 可关闭它：

```text
https://your-brain-host/mcp/my_space_001
```

Client 通过 URL path 选择目标记忆空间，并使用与 REST 相同的 CWT 或 space token：`Authorization: Bearer <token>`。这适合公司内部多用户智能体平台：为每位员工分配一个 Brain space，员工的智能体通过 MCP 连接自己的空间。

Anda Brain 也可作为本地 MCP stdio server 运行：

```bash
MCP_AUTH_TOKEN="$SPACE_TOKEN" \
  anda_brain mcp --space-id my_space_001 local --db ./data
```

两种 MCP 模式都复用 HTTP 服务的模型、认证和存储配置，并共享其模型并发预算。stdio 模式的嵌套 storage 子命令可省略（内存开发模式），也可以使用 `local --db ./data` 持久化到本地，或使用 `aws --bucket ... --region ...` 连接 S3。

| Tool | Input | Output | Scope |
| ---- | ----- | ------ | ----- |
| `anda_brain_memory` | `{ request: MemoryRequest }` | `MemoryResponse` | recall 为 `read`；变更类为 `write` |
| `anda_brain_stage_memory_source` | `StageSourceInput` | `StagedSourceRef` | `write` |
| `anda_brain_memory_receipt` | `{ receipt_ref }` | `{ receipt, progress, result, warnings }` | `read` 凭据 |
| `anda_brain_remember_conversation` | `FormationInput` 形状（`messages`, `context`, `timestamp`） | `AgentOutput` | `write` |
| `anda_brain_recall_memory` | `RecallInput` 形状（`query`, `context`，可选 `budget`） | `AgentOutput` | `read` |
| `anda_brain_run_maintenance` | `MaintenanceInput` 形状 | `AgentOutput` | `write` |
| `anda_brain_get_space_info` | 无 | `SpaceInfo` | `read` |
| `anda_brain_get_formation_status` | 无 | `FormationStatus` | `read` |
| `anda_brain_execute_kip_readonly` | `{ command?, commands?, parameters?, dry_run? }` | `KipResponse` | `read` |
| `anda_brain_get_or_init_user` | `{ user, name? }` | `Concept` | `write` |
| `anda_brain_list_conversations` | `{ collection?, cursor?, limit? }` | `{ conversations, next_cursor }` | `read` |
| `anda_brain_get_conversation` | `{ conversation_id, collection?, delta?, messages_offset?, artifacts_offset? }` | `Conversation` 或 `ConversationDelta` | `read` |
| `anda_brain_get_attention` | `{ cursor?, limit? }` | `AttentionPage` | 运行时：经验证的 `read` + 映射 |
| `anda_brain_respond_attention` | `{ id, response: AttentionResponse }` | `ResponseReceipt` | 运行时：经验证的 `write` + 映射 |
| `anda_brain_get_runtime_status` | 无 | `RuntimeStatus` | 运行时：经验证的 `read` |
| `anda_brain_wiki_search` | wiki 查询、过滤条件和结果上限 | `WikiSearchOutput` | `read` |
| `anda_brain_wiki_read` | 文档 ID 和读取选择器 | `WikiReadOutput` | `read` |
| `anda_brain_wiki_commit` | 文档字段和完整 Markdown 内容 | `WikiCommitOutput` | `write` |
| `anda_brain_wiki_verify` | citation URI 或显式引用字段 | `WikiVerifyOutput` | `read` |

MCP 只读 KIP 工具使用 `commands`，并为批量读取补上 independent 执行模式。HTTP `/execute_kip_readonly` 使用 `operations`，批量请求须显式提供 `execution.mode`。Wiki MCP 工具在启用 `wiki` feature 时提供；服务二进制始终启用该 feature。三个运行时工具遵循[运行时接口](#authenticated-runtime-inbox-and-observations)的规则；观察者写入从不作为模型工具提供。所有工具 schema 都不使用 `oneOf`、`anyOf` 或 `allOf`：备选类型写成类型列表，带标签的变体写成一个带 `enum` 判别字段的对象，跨字段规则在解析参数时检查。

当设置了 `ED25519_PUBKEYS` 时，远程 MCP 客户端需要携带 `Authorization` bearer token；stdio 模式请通过 `MCP_AUTH_TOKEN` 或 `--mcp-auth-token` 配置 CWT 或 space token。`read` 工具也可无 token 访问 public space。远程 MCP 经过公司域名或反向代理暴露时，请设置 `MCP_HTTP_ALLOWED_HOSTS`（浏览器客户端还需 `MCP_HTTP_ALLOWED_ORIGINS`）。本地 stdio 开发可用 `--mcp-auto-create-space` 自动创建目标 space；远程开发可用 `MCP_HTTP_AUTO_CREATE_SPACE=true`，但在远程自动创建不存在的 space 前，必须配置好 `ED25519_PUBKEYS`，且客户端需提供该 space 拥有 `write` 范围的 CWT。

---

## 6) 错误语义

- 认证失败：HTTP `401`，响应体为 `RpcError`
- 参数错误：HTTP `400`，响应体为 `RpcError`；`RpcError.message` 为可读的纯文本，而非带引号的调试格式
- 无权访问：HTTP `403`；空间或 wiki 文档不存在：HTTP `404`
- Wiki 提交冲突：HTTP `409`，当前版本位于 `RpcError.data.current_version`；wiki 内容超限：HTTP `413`
- 模型请求限流：HTTP `429`；全局 HTTP 限流：HTTP `503`。这些中间件响应是纯文本。
- 成功时：HTTP `200`，响应体通常为 `RpcResponse<T>`
- 处理器错误即使在 `Accept` 请求 CBOR 或 Markdown 时也返回 JSON；未匹配路由和中间件错误可能是纯文本或空响应体。只有成功响应体遵循 `Accept`。
- KIP 请求可能返回 HTTP `200`，但 `KipResponse.status` 或某项操作的 `status` 为 `failed`；必须检查两层 KIP 状态。
- MCP 工具沿用同一分类：调用方可修复的失败以 JSON-RPC `invalid_params`/`invalid_request` 返回（wiki 提交冲突携带与 HTTP `409` 相同的 `data.current_version` 重试载荷），只有真正的内部错误才用 `internal_error`

---

## 7) TypeScript 类型定义

```ts
export type TokenScope = 'read' | 'write' | '*';

export interface RpcError {
  message: string;
  data?: unknown;
}

export interface RpcResponse<T> {
  result?: T;
  error?: RpcError;
  next_cursor?: string;
}

export interface InputContext {
  counterparty?: string;
  agent?: string;
  source?: string; // 来源线程/渠道；不是提交或消息的去重键
  topic?: string;
}

export type MessageRole = 'system' | 'user' | 'assistant' | 'tool';

export type MessageContentPart =
  | string
  | {
      type: string;
      text?: string;
      [k: string]: unknown;
    };

export interface Message {
  role: MessageRole;
  content: string | MessageContentPart[];
  name?: string;  // user 或 tool 的名称
  user?: string;  // user ID
  timestamp?: number; // Unix timestamp in milliseconds
}

export interface FormationInput {
  messages: Message[]; // 至少包含一条非空消息（否则 400）
  context?: InputContext;
  timestamp?: string; // RFC 3339；规范为 UTC 毫秒格式；无法解析或精度超过毫秒返回 400；缺省时使用接收时间
}

export interface RecallInput {
  query: string; // 不能为空/纯空白（否则 400）
  context?: InputContext;
  budget?: RecallBudget | null;
}

export interface RecallBudget {
  // 指 tiktoken-rs 0.12 版本线的 o200k 编码。旧写法
  // 'o200k_base@tiktoken-rs-0.12.0' 是同一编码，仍被接受。
  tokenizer?: 'o200k_base@tiktoken-rs-0.12';
  max_tokens?: number; // 1–65536；显式启用后的默认值为 4096
  context_tokens?: number; // 1–131072；默认 49152；整次规划输入规范序列化的累计上限
}

export interface MemoryPolicy {
  version?: number;
  memory_strength_decay_factor?: number; // 已弃用；仅为兼容已保存策略保留，不生效
  recall_reinforcement?: number; // retained for stored-policy compatibility; inert
  correction_penalty?: number; // retained for stored-policy compatibility; inert
  decay_floor?: number; // 已弃用；仅为兼容已保存策略保留，不生效
  stale_event_threshold_days?: number;
  unconsolidated_max_backlog?: number;
  orphan_max_count?: number;
  self_test_queries_per_cycle?: number;
  self_test_token_budget?: number;
  recall_search_threshold?: number; // declared but not consumed yet
  recall_max_rounds?: number;
  recall_budget?: RecallBudget | null;
  shadow_replay_sample?: number;
}

export interface MemoryCitation {
  entity: string;
  type?: string;
  name?: string;
  confidence?: number;
  source?: string;
  created_at?: string;
}

export interface RecallBudgetReceipt {
  tokenizer: string;
  token_limit: number;
  tokens: number;
  context_token_limit: number;
}

export interface RecallOutput {
  answer: string;
  found: boolean;
  uncertainty?: number;
  memories?: MemoryCitation[];
  conversation?: number;
  usage: Usage;
  failed_reason?: string;
  memory_budget?: RecallBudgetReceipt;
}

export interface ProbeInput {
  query: string;
  limit?: number;
}

export interface ProbeOutput {
  found: boolean;
  negative_cached: boolean;
  search_exhaustive?: boolean;
  hits?: MemoryCitation[];
}

export interface MemoryPinInput {
  entity: string; // 图谱实体 ID，例如 C-7、P-3、A-2
  pinned?: boolean; // default true
}

export interface MemoryPinOutput {
  entity: string;
  pinned: boolean;
  updated: number; // 更改的 retention 记录数
}

export interface MemoryForgetInput {
  entities: string[];
  dry_run?: boolean; // default false; inspect a dry run before deletion
}

export interface MemoryForgetEntity {
  entity: string;
  existed: boolean;
  error?: string;
}

export interface MemoryForgetReport {
  dry_run: boolean;
  deleted_concepts: number;
  deleted_propositions: number;
  deleted_assertions: number;
  deleted_evidence: number;
  deleted_activities: number;
  entities?: MemoryForgetEntity[];
}

export interface MemoryMetrics {
  recalls_completed: number;
  entities_recalled: number;
  probe_hits: number;
  probe_misses: number;
  negative_cache_hits: number;
  self_test_tested: number;
  self_test_grounded: number;
  reencode_tasks: number;
  corrections: number;
  uncertainty_reports: number;
  uncertainty_sum: number;
  forgotten_entities: number;
  updated_at: number;
}

export interface MemoryGraphCounters {
  concepts: number;
  propositions: number;
  unconsolidated?: number;
  orphans?: number;
  predicate_types?: number;
  as_of?: number;
}

export interface WatchSettlement {
  fired: number;
  conflicted: number;
  disarmed: number;
  deferred: number;
  error?: string;
}

export interface SkillSettlement {
  unsupported_reason?: string;
  graded: number;
  transitions: number;
  conflicted: number;
  error?: string;
}

export interface MemorySettlementReport {
  settled_at: number;
  revised_roots?: unknown[];
  exposures?: { element_id: string; retrieved: number; used: number; last_snapshot_seq: number }[]; // 曝光日志的下一批（Spec §66.8）：强化的输入
  exposures_truncated?: boolean;
  new_corrections: number;
  watches: WatchSettlement;
  commitments: { due: number; raised: number; error?: string }; // 以 commitment_review Activity 提起的到期 Commitment
  skills: SkillSettlement;
  correction_scan_error?: string;
  correction_scan_incomplete: boolean;
  correction_scan_through_seq: number;
  retention: { // 保留期已到的记录；主张有效期在读取时计算
    archived: number;
    held: number;
    refused: number;
    remaining: number;
    error?: string;
  };
}

export interface SelfTestReport {
  tested_at: number;
  tested: number;
  grounded: number;
  reencode_tasks: number;
  usage: Usage;
}

export interface ShadowEvalInput {
  policy: MemoryPolicy;
  replay_sample?: number; // default from policy, at most 16
}

export interface ShadowReport {
  compared_at: number;
  replayed: number;
  baseline_wins: number;
  candidate_wins: number;
  ties: number;
  judge_errors: number;
  candidate_policy: MemoryPolicy;
  usage: Usage;
  samples?: { query: string; winner: 'baseline' | 'candidate' | 'tie' | 'error'; reason?: string }[];
}

export interface MemoryStatus {
  metrics: MemoryMetrics;
  groundability?: number;
  probe_hit_rate?: number;
  correction_rate?: number;
  avg_uncertainty?: number;
  maintenance_tokens_per_recall?: number;
  graph: MemoryGraphCounters;
  last_settlement?: MemorySettlementReport;
  last_self_test?: SelfTestReport;
  last_shadow?: ShadowReport;
  last_schema_audit?: { audited_at: number; predicates?: Record<string, number> };
}

export interface MaintenanceParameters {
  stale_event_threshold_days?: number; // [1, 365]
  memory_strength_decay_factor?: number; // 已弃用并被忽略；仍校验 (0, 1]；旧名 confidence_decay_factor 仍被接受
  unconsolidated_max_backlog?: number; // [1, 10000]；旧名 unsorted_max_backlog 仍被接受
  orphan_max_count?: number; // [1, 10000]
}

export interface MaintenanceInput {
  trigger?: 'scheduled' | 'threshold' | 'on_demand';
  scope?: 'full' | 'quick' | 'daydream'; // 默认 'daydream'
  timestamp?: string; // 规范 UTC：YYYY-MM-DDTHH:mm:ss.SSSZ
  parameters?: MaintenanceParameters;
}

export interface AddSpaceTokenInput {
  scope: TokenScope; // 铸造 "*" 需要 "*" scope 的 CWT
  name: string; // 必填，空间内唯一
  expires_at?: number; // Unix timestamp in milliseconds
  labels?: string[]; // wiki ACL 标签；缺省 = 不受限，[] = 仅无标签内容
}

export interface RevokeSpaceTokenInput {
  token?: string; // 完整 token 值……
  name?: string; // ……或唯一 token 名称（两者必填其一）
}

export interface UpdateSpaceInput {
  name?: string;
  description?: string;
  public?: boolean;
  wiki_digest?: boolean; // 开启 WikiDigest 图谱蒸馏（默认关闭）
  wiki_audit_reads?: boolean; // 外部 wiki 读操作写审计事件（默认关闭）
  wiki_acl_defaults?: Record<string, string>; // namespace -> 默认 ACL 标签
  memory_policy?: MemoryPolicy; // 替换空间策略；省略的成员使用服务端默认值
}

export interface FormationRestartInput {
  conversation: number;
}

export interface CreateOrUpdateSpaceInput {
  user: string;
  space_id: string;
  tier: number;
}

export interface GetOrInitUserInput {
  user: string;
  name?: string;
}

// ── Wiki：版本化参考文档与可校验引用 ──────────────────────────────

export type WikiDocStatus = 'active' | 'archived';
export type WikiSearchMode = 'chunks' | 'docs';

export interface WikiCommitInput {
  doc_id?: number; // 缺省 = 创建新文档
  parent_version?: number; // 更新必填（CAS）；过期返回 409
  namespace?: string; // 默认 "default"
  slug?: string; // 展示用；缺省从标题派生
  title: string;
  content: string; // 全量 Markdown（非 diff）；规范化后 ≤ 1 MiB
  tags?: string[]; // 缺省 = 更新时保持原值
  acl_label?: string; // 缺省 = 保持/继承 namespace 默认；"" 清除
  source_uri?: string; // 缺省 = 保持原值
  message?: string; // 提交说明
  metadata?: Record<string, unknown>; // 缺省 = 保持原值
}

export interface WikiDocInfo {
  id: number;
  namespace: string;
  slug: string;
  title: string;
  status: WikiDocStatus;
  current_version: number;
  current_checksum: string; // "sha3-256:..."
  tags: string[];
  acl_label?: string;
  source_uri?: string;
  metadata?: Record<string, unknown>;
  created_by: string;
  updated_by: string;
  created_at: number;
  updated_at: number;
}

export interface WikiVersionInfo {
  id: number;
  doc_id: number;
  parent_version?: number;
  checksum: string;
  size: number;
  author: string;
  message?: string;
  created_at: number;
}

export interface WikiCommitOutput {
  doc: WikiDocInfo;
  version: WikiVersionInfo;
  chunks: number;
  created: boolean;
  idempotent: boolean; // true = 内容未变，零写入
}

export interface WikiSearchInput {
  query: string; // BM25 关键词：术语、产品名、错误码优于整句
  namespaces?: string[];
  doc_ids?: number[];
  tags?: string[];
  top_k?: number; // 1-50，默认 8
  mode?: WikiSearchMode; // 'docs' = 每篇文档只返回最佳命中
  expand?: number; // 0-2 邻域扩展；引用范围相应扩大
}

export interface WikiCitation {
  uri: string; // wiki://{space}/{doc_id}@{version_id}#{start}-{end}
  doc_id: number;
  version_id: number;
  chunk_id: number;
  heading_path: string[];
  anchor: string; // 稳定章节锚点，可用于按节读取
  byte_range: [number, number];
  checksum: string; // 可经 /wiki/verify 校验
  quote: string;
}

export interface WikiHit {
  text: string;
  doc_title: string;
  heading_path: string[];
  citation: WikiCitation;
}

export interface WikiSearchOutput {
  hits: WikiHit[];
  total_docs_matched: number;
}

export type WikiSelector =
  | { type: 'toc' }
  | { type: 'section'; anchor: string }
  | { type: 'range'; start: number; end: number }
  | { type: 'full' };

export interface WikiReadInput {
  doc_id: number;
  version?: number; // 读取历史版本（time-travel）
  selector?: WikiSelector; // 默认 { type: 'full' }
}

export interface WikiTocEntry {
  anchor: string;
  heading_path: string[];
  byte_start: number;
  byte_end: number;
}

export interface WikiReadOutput {
  doc_id: number;
  version_id: number;
  is_current: boolean;
  title: string;
  status: WikiDocStatus;
  checksum: string;
  size: number;
  toc?: WikiTocEntry[]; // selector 为 'toc' 时返回
  content?: string; // section/range/full 时返回
  byte_range?: [number, number];
  truncated: boolean; // 全文读取有上限（256 KiB）
}

export interface WikiVerifyInput {
  uri?: string; // wiki:// 引用 URI；或使用下方显式字段
  doc_id?: number;
  version_id?: number;
  byte_range?: [number, number];
  checksum?: string; // 提供时与重算校验和比对
}

export type WikiVerifyStatus = 'valid' | 'superseded' | 'invalid' | 'not_found';

export interface WikiVerifyOutput {
  status: WikiVerifyStatus; // 'superseded' = 内容完好但已有新版本
  current_version?: number;
  checksum?: string; // 从不可变内容重算
  quote?: string;
}

export interface WikiBundleEntry {
  path: string; // bundle 相对路径，如 "guides/setup.md"
  content: string;
}

export interface WikiImportInput {
  entries: WikiBundleEntry[]; // OKF v0.1 bundle 文件（Markdown + YAML frontmatter）
  namespace?: string; // 默认 "default"；bundle 以 namespace 为单位往返
}

export type WikiImportStatus = 'created' | 'updated' | 'unchanged';

export interface WikiImportOutput {
  created: number;
  updated: number;
  unchanged: number; // checksum 幂等：重复导入零版本膨胀
  docs: { path: string; doc_id: number; version_id: number; status: WikiImportStatus }[];
  skipped?: { path: string; reason: string }[];
}

export interface WikiExportOutput {
  namespace: string;
  entries: WikiBundleEntry[]; // concept .md 文件 + index.md + manifest.json
  docs: number;
}

export interface WikiEventInfo {
  id: number;
  // DocCreated | VersionCommitted | DocArchived | DocRestored | OrphanSwept
  // | CitationVerifyFailed | ImportCompleted | ExportCompleted
  // | DigestExtracted | DigestFailed | WikiQueried | WikiRead | StaleReport | EventsPruned
  kind: string;
  doc_id?: number;
  version_id?: number;
  actor: string;
  detail?: Record<string, unknown>;
  created_at: number;
}

export interface WikiDigestReport {
  digested: number; // 已提取或复用已有结果的文档代数
  facts: number; // 本轮处理的文档账本中保留的断言数，不表示事实完整性
  superseded: number; // 经明确核验或来源撤回而 retract 的本来源断言数
  skipped: number; // 无需提取的已归档、带标签或评测文档
  failed: number; // 失败且仍在待处理队列中的文档；再次调用可重试
  citations_checked: number;
  citations_invalid: number;
  usage: Usage;
}

export interface McpServerConfig {
  space_id: string;
  auth_token?: string;
  auto_create_space?: boolean;
  auto_create_tier?: number;
}

export interface McpHttpServerConfig {
  path_prefix?: string; // 默认 "/mcp"; client 连接 {path_prefix}/{space_id}
  allowed_hosts?: string[]; // rmcp 默认只允许 loopback；公司域名需要显式配置
  allowed_origins?: string[]; // 浏览器型 MCP client 使用
  auto_create_space?: boolean;
  auto_create_tier?: number;
}

export interface Concept {
  id: string;
  kind: 'concept';
  space_id?: string;
  schema_ref?: string;
  key?: string;
  name?: string;
  canonical_id?: string;
  aliases?: string[];
  attributes?: Record<string, unknown>;
  facets?: Record<string, Record<string, unknown>>;
  retention?: { retention_class?: string; expires_at?: string; legal_hold?: boolean };
  _system?: Record<string, unknown>;
}

export interface ModelConfig {
  family: string; // "gemini", "anthropic", "openai", "deepseek", "mimo" etc.
  model: string;
  api_base: string;
  api_key: string;
  disabled?: boolean;
  label?: string;
  effort?: 'minimal' | 'low' | 'medium' | 'high' | 'max';
  bearer_auth?: boolean;
  stream?: boolean;
  context_window?: number;
  max_output?: number;
}

export interface SpaceTier {
  tier: number;
  updated_at: number; // Unix timestamp in milliseconds
}

export interface SpaceToken {
  token: string;
  name: string;
  scope: TokenScope;
  usage: number;
  created_at: number; // Unix timestamp in milliseconds
  updated_at: number; // Unix timestamp in milliseconds
  expires_at?: number; // Unix timestamp in milliseconds
  labels?: string[]; // wiki ACL 标签：[] 仅可见无标签内容；缺省不受限
}

export interface StorageStats {
  [k: string]: number | string | boolean | null;
}

export interface SpaceInfo {
  id: string;
  name?: string;
  description?: string;
  owner: string;
  db_stats: StorageStats;
  concepts: number;
  propositions: number;
  conversations: number;
  public: boolean;
  tier: SpaceTier;
  formation_usage: Usage;
  recall_usage: Usage;
  maintenance_usage: Usage;
  formation_processed_id: number;
  maintenance_processed_id: number;
  maintenance_at: MaintenanceAt;
  memory_interface?: MemoryDescriptor; // 本 Space 提供的 Memory Interface
  wiki_docs: number;
  wiki_chunks: number;
  wiki_versions: number;
  wiki_queries: number;
  wiki_digested: number; // 观测用的版本高水位，不代表所有文档都已处理
  wiki_stale_docs: number; // 最近一次 housekeeping 陈旧扫描结果
}

export interface FormationStatus {
  id: string;
  concepts: number;
  propositions: number;
  conversations: number;
  formation_processing: boolean;
  maintenance_processing: boolean;
  formation_processed_id: number;
  maintenance_processed_id: number;
  maintenance_at: MaintenanceAt;
}

export interface MaintenanceAt {
  daydream: number;
  full: number;
  quick: number;
  /** 最近一次 maintenance 任务的启动时间（unix 毫秒），0 表示尚未启动过。 */
  start_at: number;
}

export interface Usage {
  /** 发送给 LLM 的输入 token 数。 */
  input_tokens: number;
  /** 从 LLM 接收的输出 token 数。 */
  output_tokens: number;
  /** 执行过程中命中缓存的 token 数。 */
  cached_tokens: number;
  /** 对模型、agent 或工具发起的请求次数。 */
  requests: number;
}

export interface AgentOutput {
  content: string;
  conversation?: number;
  failed_reason?: string;
  usage?: Usage;
  model?: string;
  [k: string]: unknown;
}

export type ConversationStatus =
  | 'submitted'
  | 'working'
  | 'idle'
  | 'completed'
  | 'failed'
  | 'cancelled';

export interface Conversation {
  _id: number;
  user: string;
  thread?: string;
  label?: string;
  messages: Message[];
  resources: unknown[];
  artifacts: unknown[];
  status: ConversationStatus;
  failed_reason?: string | null;
  period: number;
  created_at: number;
  updated_at: number;
  usage: Usage;
  steering_messages?: string[];
  follow_up_messages?: string[];
  child?: number;
  extra?: unknown;
  ancestors?: number[];
}

export interface ConversationDelta {
  _id: number;
  messages: unknown[];
  artifacts: unknown[];
  status: ConversationStatus;
  usage: Usage;
  failed_reason?: string | null;
  updated_at: number;
  child?: number | null;
}

export interface ServiceInfo {
  name: string;
  version: string;
  sharding: number;
  description: string;
  memory_interface: MemoryDescriptor; // 不含 default_space
}

export interface MemoryDescriptor {
  kip_memory: '2.0';
  bundles: ('memory_basic' | 'memory_experience' | 'memory_learning')[];
  default_scope?: { task_ref?: string; context_refs?: string[] };
  default_budget: { max_output_tokens: number; deadline_ms: number };
  tokenizer: string;
  minimum_response_tokens: number;
  default_space?: { id: string };
}

export type KipOperation = string | {
  op_id?: string;
  language?: 'KQL' | 'KML' | 'META'; // 声明仅供参考；只读限制以解析后的命令为准
  command?: string;
  ast?: unknown;
  parameters?: Record<string, unknown>;
  idempotency_key?: string;
  options?: { extensions?: Record<string, unknown> };
  extensions?: Record<string, unknown>;
};

export interface KipRequest {
  command?: string; // 单条命令；与 `operations` 互斥
  operations?: KipOperation[]; // 一次往返执行多条命令
  execution?: { mode: 'independent' | 'sequence' | 'atomic'; on_error?: 'stop' | 'continue'; isolation?: string; idempotency_key?: string; extensions?: Record<string, unknown> }; // 多于一条操作时必填
  read?: { snapshot_token?: string; extensions?: Record<string, unknown> }; // 把所有操作绑定到同一读取坐标
  parameters?: Record<string, unknown>; // 绑定到命令中 `:placeholder` 的值
  dry_run?: boolean; // 仅校验与规划，不提交
}

export interface KipError {
  code: string; // 注册表名称，如 "NotFoundOrNotVisible"，不再是数字码
  message: string;
  category?: string;
  hint?: string;
  retry?: { class: string; after_ms?: number };
  details?: unknown;
}

export interface KipOperationResult<T> {
  op_id?: string;
  status: 'succeeded' | 'failed' | 'skipped' | 'rolled_back' | 'no_effect';
  result?: T;
  context?: unknown;
  error?: KipError;
  warnings?: unknown[];
  next_cursor?: string;
  receipt?: unknown;
  extensions?: Record<string, unknown>;
}

export interface KipResponse<T> {
  kip: '2.0';
  request_id?: string;
  status: 'succeeded' | 'failed' | 'partial' | 'outcome_unknown';
  results: KipOperationResult<T>[];
  execution?: unknown;
  context?: unknown;
  snapshot?: { space_id?: string; snapshot_seq: number; schema_environment_version?: number; snapshot_token?: string; extensions?: Record<string, unknown> };
  receipt?: unknown;
  warnings?: unknown[];
  next_cursor?: string;
  error?: KipError; // 仅当请求在进入 operations 之前就失败时才设置
  extensions?: Record<string, unknown>;
}
```

> **KIP 2.0。** 普通错误在 operation 层，信封错误才在请求层，客户端两处都要读。
> `outcome_unknown` 既不是成功也不是失败：写入可能已经提交，正确做法是按幂等键
> 查事务，而不是当作没发生过重发一次。

---

## 8) 前端调用示例（TS）

```ts
async function rpcPost<TReq, TRes>(
  url: string,
  body: TReq,
  token?: string
): Promise<RpcResponse<TRes>> {
  const res = await fetch(url, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Accept: 'application/json',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
    body: JSON.stringify(body),
  });

  const responseText = await res.text();
  if (!res.ok) {
    let message = responseText || `HTTP ${res.status}`;
    try {
      const error = JSON.parse(responseText) as RpcError;
      if (error.message) message = error.message;
    } catch { /* 中间件错误可能是纯文本 */ }
    throw new Error(`HTTP ${res.status}: ${message}`);
  }
  return JSON.parse(responseText) as RpcResponse<TRes>;
}

// Recall
const recall = await rpcPost<RecallInput, AgentOutput>(
  '/v1/my_space_001/recall',
  { query: '这个用户的偏好是什么？', context: { counterparty: 'user_1' } },
  'YOUR_TOKEN'
);

if (recall.error) {
  console.error(recall.error.message);
} else {
  console.log(recall.result?.content);
}
```

---

## 9) Rust 宿主接口

以下是供嵌入宿主使用的受信 Rust 接口，不新增 HTTP 或 MCP 路由，也不授予模型任何权限。

<a id="trusted-host-memory-product-contracts"></a>

### 9.1 可信宿主记忆产品合同

Rust `product` 模块提供由 Assertion 支撑的 `MemoryRecord`、稳定修订、明确的立场/语义生命周期/存储状态，以及 Evidence 的 typed 来源引用。`Space::product_records`、`product_record`、`product_source` 不负责终端用户认证；嵌入宿主在返回记录、预览、派生 ID 或来源引用前必须检查所有者及来源权限。摘要匹配证明来源关联，不证明推断正确。

`Space::ingest_product` 接收有界、可信的 `SourceIdentity`，包含父会话/会话链标识，自然语言输入不能指定它。`product_prepare`、`product_commit`、`product_change`、`product_discard` 按 caller/operation 保存不可变请求，绑定修订、预览摘要及十分钟有效期。`ChangeKind` 指明变更写入哪一种历史（KIP Spec §14.2、Memory Interface §4）：

- `Correct`：调用者自己的主张写错了。新主张带用户陈述 Evidence，supersede 旧 Assertion，并保留其原有的世界时间区间（缺失的起点写成 `{latest: <原 asserted_at>}`），由 `belief_revision` Activity 记录；旧 Assertion 状态为 `superseded`。
- `WorldChange`：世界变了。从现在起写一条新主张，时序继承结束旧值；旧值保持 `active`，仍回答它所在时段的问题。

两种修订都沿用该记录的 `context_refs`（`MemoryRecord` 已暴露）：跨上下文集合的 supersession 会报 `SupersessionMismatch`（规范 §14.2）；新值若不在旧值的上下文集合里，会另起一条继承线而结束不了旧值（规范 §25.4）。
- `Misrecorded`：Brain 记下了调用者从未说过的话。这是 recording repair，通过 Memory Interface 的 `revise` 意图（`change_kind: "misrecorded"`）执行（见 [Memory Interface](#memory-interface)）；这个按值修订的产品 API 返回 `unsupported_capability`，绝不写成更正或世界变化。

以上都不改写原 Concept 名称，也不伪造跨 Proposition 的 supersession。撤销是新的条件变更。

`Suppress` 归档，`Delete` 清除已声明的有界集合：选定 Proposition/Assertions、引用输入及已记录的反向依赖。未知来源、Concept 级联、保留约束和超过 128 项的集合会被拒绝。写入前先持久接受来源排除与处理 epoch；原生任务不因 API 等待者取消而丢失，Space 重载开放前会恢复已接受操作。旧 Formation/Maintenance/Notes 写入被版本栅栏拦截。变更清空处理 Notes 和 miss cache、停止向新上下文注入旧处理历史，并限制自动 KIP 读取当前 active 数据；可信所有者审计接口保持独立。Recall 返回上下文前重新核对捕获的 epoch。

受管理变更也会阻止预算版 Recall 注入旧历史，并在超时或轮次耗尽返回前检查处理版本。变更前开始的分页须从首页重试，变更后的分页仍可继续。删除会清理已清除内容在操作预览／更正正文中的副本，并废弃受影响的未提交意图，同时保留操作标识和摘要。若独立更正的声明及 Evidence 仍然存在，其来源正文会保留。更正来源读取会检查 Evidence 尚未清除且正文摘要一致。

移除不清理宿主的原聊天、文件、日志、备份、其他独立图谱记录、已交付的上下文或服务商副本。最小来源标识与摘要用于阻止重放。用不含当前排除信息的旧备份覆盖数据库不是保留删除语义的回滚方式。宿主也必须清空自己的注入 Notes，并防止后续对话链再次导入已排除来源。

`MemoryRuntime::{create_record_watch,record_watch,cancel_record_watch}` 使用不可伪造的已认证 `RuntimeCaller` 实现收件人拥有的窄范围订阅。创建持久记录身份，仅启用初始 generation；配置的控制器只获准归档该 Watch，持久标记保证不重新授予已撤销权限。取消使用原生归档，不伪造受保护运行状态；重试不重新启用。已投递的问题是独立工作，仍可见。它们是 Rust 接口，不新增通用模型工具或原生 HTTP 产品管理路由。

`RuntimeConfig::validate` 静态校验不加载 Space、不运行模型、不探测业务服务、不配置授权。可选学习运行时的 `product_readiness` 表示已安装隔离工作流的准备度，区分服务缺失、身份摘要不匹配、校准未审阅及批准门槛。ready 不授予业务部署权限，每项工作的服务与原生权限检查仍必须执行。

Anda Bot 通过已发布的 crate 使用这些合同，并保持 DB/KIP/Core 的单一类型身份。机制测试不证明真实学习收益或完整成本计量。

### 9.2 可信学习运行时（`learning` feature）

启用 `learning` feature 后，`Space::learning()` 提供显式注册、冻结 cohort、
有界 `drive` 步骤、重启后发现工作及独立认证的 Outcome 接收。原生 lease、当前
可执行权限、依赖有效性和 policy pin 共同约束派发。这些宿主 API 不增加 HTTP/MCP
路由或模型写权限；收齐 cohort 不会采纳 Skill。`settle(job_id)` 在固定 cutoff 后
重算原生账本，原子提交裁决与 standing。`reviews()` 和 `enroll_review()` 提供保留
原始采纳依据的持久复核；`submit_safety_signal()` 接受独立认证的安全撤销信号。
`bind_application_context()` 接受短时有效的可信宿主环境观测，`procedure_status()`
及 Recall 内部的 `check_procedure_status` 工具只读检查当前推荐条件，不授予执行权限。
条件无法验证或复核到期时停止推荐。已配置 learning 的 Space 不允许通过 fork/snapshot
复制操作 journal。详见 [运行时](README.md#native-learning-contracts) 和
[生命周期与恢复](README.md#native-learning-contracts)。

### 9.3 隔离实验与 MIB 宿主（`experiments` feature）

相邻 Anda Bot 的 `mib` feature 在本机提供独立的 `/mib-agent/v0.1` 和
`/mib-memory/v0.1` 协议；它们不属于本生产 Brain API 的路由。宿主使用
`experiments` 实现隔离状态、完成屏障、单调业务时间和清理，详见
[接入](README.md#mib-integration)。适配器不声明在线学习能力；
缺少 provider 或 observer 遥测的成本仍明确标为不完整。

Rust 工厂 `Experiment::create_with_recall_budget` 在运行对外可用前持久化强制
Recall 预算。`audit_procedures()` 返回有界、只读的原生程序清单；截断计数不能证明
不存在。仅 Bot MIB 的 `learning_audit` 扩展将该清单交给评测侧，不进入业务模型。
它不启用学习，也不赋予执行权限。详见 [验证](README.md#mib-integration)。

### 9.4 实例配置与离线回归

已退役 Rust `anda_brain::eval` API 与 `eval` CLI（含 optimizer/miner 参数）。
MIB 提供公开产品回归 profile；自测、shadow 诊断、probe、引用与账本继续作为线上
工具。运行策略使用各 Space 持久化的 `MemoryPolicy`。可信 Rust 宿主可以在共享或
打开 Space 前调用 `AppState::with_agent_prompts(AgentPrompts)` 提供不可变的部署
段：每段以 `# A.` 开头、最多 128 KiB，编译参考前缀保持原样。这不是 HTTP/MCP
提示操作。详见 [迁移](README.md#offline-regression-and-instance-configuration)。

---

## 10) 执行与资源限制

Formation 与 Maintenance 共用每个 Space 的写入准入 guard，覆盖 Maintenance
前置的确定性结算。Formation 持有 guard 时，显式 Rust/HTTP/MCP Maintenance
请求会被拒绝；Maintenance 结束后恢复排队的 Formation。Formation 的阈值触发
在启动下一个 worker 前移交 guard。

`LLM_MAX_CONCURRENCY` 限制通过 Brain 模型注册表发起的跨 Space 在途模型调用，
包括后台 Formation/Maintenance、上下文压缩、WikiDigest、自检和 shadow judge。
HTTP/MCP 请求准入使用同容量但独立的 semaphore：超额请求仍返回 HTTP 429
（或 MCP busy 错误），已准入的模型调用可以等待额度，等待可取消。显式绑定的
语义 Watch 与 learning provider 继续使用各自配置的预算。这是并发限制，
不是累计 token 或金额预算。

Space 关闭会取消模型等待、停止后台准入并排空已准入的原生结算写入。
驱逐关闭失败时保留原 owner 供重试；关闭完成前该 Space 的访问可能暂时失败。
其数据库关闭期间，其他 Space 仍可访问。

自检独立记录 `last_self_test_at`，未被使用的记忆在 30 天后可以复测。
`self_test_token_budget` 使用 Recall 固定 tokenizer 计量实际宿主 instructions/prompt，
并预留四分之一额度（最多 4096 tokens）作为请求的输出上限。放不下的候选不会发送。
提供商封装和分词可能不同；不支持输出上限时该诊断失败，缺失的提供商 usage
仍是未知值。这不构成提供商账单保证。
