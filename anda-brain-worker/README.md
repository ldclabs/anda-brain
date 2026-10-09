# Anda Brain Worker

[逐提交移植核对](PORTING_AUDIT.md) · [记忆产品宿主契约](PRODUCT_cn.md) · [English host contracts](PRODUCT.md)

这是一个参考 Rust 版 `anda_brain`、基于 `@ldclabs/kip-do` 实现的简化版 Cloudflare Worker，使用 **KIP 2.0**。它保留三条核心认知链路：

- Formation：把对话提炼成 KIP KML 事务，写入长期图谱记忆。
- Recall：把自然语言问题规划为只读 KIP 查询，再根据图谱证据生成答案。
- Maintenance：检查近期 Event / SleepTask / 最久未动的记忆，由模型规划合并、更新或归档。

每个 `space_id` 映射到一个独立的 SQLite Durable Object。KIP 图谱、原子事务和模式包由 `@ldclabs/kip-do` 提供；自然语言规划和答案合成使用 Workers AI。

不再有批量代谢：记忆强度在读取时按基值、锚点与钉住的策略计算，settlement 不写强度。
维护模型会收到实际 settlement。

## KIP 2.0 更新

本 Worker 对齐 KIP `11a82ec` 与 `kip://profiles/cognitive-memory@2.0.0`（修订
`sha256:734aa0fd…`，只能靠摘要区分修订），使用 `@ldclabs/kip-do` 0.14。用 2.1.0 草案
激活过的 Space 不做迁移。世界变化记为一条新 Assertion，由时序继承结束旧值；主张的
`at` 取所引用消息的观察时间；选项按类别定型，Profile 没有 `Preference` 类型；衰减在
读取时计算；新增 `GET /v1/{space}/memory/attention` 注意力召回。Skill 行为保存为不可变的
`SkillRevision`，并用 `current_trial` / `current_evaluation` 指针；Watch 进度和任务租约
通过 Nexus 的受保护接口维护。旧的 family 成功率晋升规则已移除；未配置独立观察者、冻结
试验和可重放评估时，程序候选保持未验证，`skills.unsupported_reason` 明确报告该边界。现有
Brain API 保持可用。两个适配器现在还在 `memory_basic` 级别提供 KIP Memory Interface
（见下文「Memory Interface」），不声明 `memory_experience` 与 `memory_learning`。

## KIP 2.0 意味着什么

1.x 把含义、信念、证据、来源和模式塞在同一张图里；2.0 把它们分开，而其余区别都来自同一条：**一个 Proposition 存在，不等于它为真**。落到这个 Worker 上：

- 一条事实是「truth-neutral 的 Proposition」加上「携带某个 actor 立场、模式、置信度与 Evidence 的 Assertion」。主张写错了是**新增一条 Assertion 并 SUPERSEDE**；世界变了是**从变化时刻起的一条新 Assertion**，由时序继承结束旧值。两者都绝不改写原有记录。
- 永远不要随时间衰减 Assertion 的 confidence。衰减在读取时由 `MnemonicState` 的基值、锚点与钉住的策略计算，那是可及性，不是真假。
- 元素 id 形如 `C-7`、`P-11`、`A-3`、`E-2`、`X-1`。
- **Schema 是受保护的控制状态。** 新词汇是 Space 草稿词汇（KIP §20.16）：Formation 用 `DEFINE` 起草，宿主校验、限量并排队审阅，只有所有者能晋升，见下文。

## 依赖：kip-do 0.14

`@ldclabs/kip-do` 0.14 来自 npm，安装、测试和部署不需要同级检出：

```bash
pnpm install --frozen-lockfile
pnpm --filter @ldclabs/anda-brain-worker check
```

主动刷新 vendored KIP 提示资产时，`sync:assets` 默认使用同级 `anda-db` 源码；也可用
`ANDA_KIP_SOURCE` 指向已下载的 `anda_kip` crate 目录，按发布版本同步。

## 与完整版的边界

这个实现面向小型 Agent、个人项目和边缘部署，不是 Rust 服务的完全移植：

| 能力 | Worker 版 |
| --- | --- |
| Formation / Recall / Maintenance | 保留，同步执行 |
| 每空间独立图谱 | SQLite Durable Object |
| KIP 2.0 查询与写入 | 保留 |
| 每空间 Schema Package（新词汇） | 保留，见下节 |
| JSON API | 保留核心入口 |
| CBOR / Markdown 协商 | 未实现 |
| 异步 Formation 队列与对话历史 | 未实现 |
| Wiki、MCP、BYOK、分级令牌 | 未实现 |
| 来源记录、审阅式更正/抑制/删除 | 受信宿主 RPC，见 [产品契约](PRODUCT_cn.md) |
| 记录 Watch | 显式接收者绑定、原生推进与取消；无后台 inbox |
| 预算化 Recall / 学习运行时 | 未实现；非空 Recall `budget` 显式拒绝，学习就绪保持不可用 |
| 自动周期维护 | 未实现；由调用方或 Cron Trigger 调用 maintenance |
| 确定性 settlement（Nexus Watch 推进 / 更正发现） | 保留，见下节；不扫盘衰减 |
| 注意力召回（`GET memory/attention`） | 保留，按提起提交的 `raised_seq` 排序，游标由调用方保存 |
| Memory Interface（`POST memory`，`memory_basic`） | 保留；Formation 在请求内完成，回执返回时已是 `available` 或 `failed`；预算按 UTF-8 字节保守限制；存储表面只有 Durable Object |
| 全文检索（`SEARCH`） | 保留，keyword 模式，见「检索」一节 |
| 派生闭包（`LIST DEPENDENTS`） | 保留，`DEPTH` 上限 8；runtime 在 settlement 里替模型走：每条新 superseded 的 Assertion 带着它的 dependents 进 `assessment.revised_roots` |
| 保留期（`SET RETENTION`） | 引擎已实现，maintenance 可以写；但没有到期清扫，Rust 服务两样都有 |
| 载荷清除（`PURGE PAYLOAD`） | 引擎已实现；maintenance 计划里和 `PURGE` 一样被拒 |
| 原子批（`execution.mode: "atomic"`） | 引擎未实现（`atomic_batch` 能力为 false），请求按 §75.3 被拒绝而不是降级成 sequence |

## Memory Interface

`POST /v1/{space}/memory` 接收一个 KIP 2.0 Memory Interface 请求（`kip_memory: "2.0"`，
`observe` / `recall` / `revise` / `feedback` / `forget` 之一），返回 Memory Interface
`Response`（不是 `{result}` 包装），失败也以 `status: "failed"` 加 KIP 错误放在 Response 里。
描述符见 `GET /` 与 `GET /v1/{space}/info` 的 `memory_interface`，与 `DESCRIBE CAPABILITIES`
一致：只有 `memory_basic`。行为与 Rust 服务相同（见 [API](../anda_brain/API_cn.md#memory-interface)），
差异如下：

- 这个 Worker 只有一个 API key，所以所有调用方同属一个句柄命名空间；该 key 同时是所有者凭据，
  可以执行 `semantic` forget。
- `POST /v1/{space}/memory/sources` 暂存来源（最多 16 条消息、256 KiB）；`observed_at` 与
  Formation 的 `timestamp` 同一规则：带时区偏移、至多毫秒精度的 RFC 3339，否则返回 400。
  `GET /v1/{space}/memory/{receipts|sources|plans}/{ref}` 读取回执进度、暂存来源与擦除计划。
- observe / revise 在请求内完成 Formation，回执返回时已是 `available` 或 `failed`；一直没有回报
  结果的回执十分钟后读为 `failed`（`OutcomeUnknown`），不会重跑。带作用域的计划若有 ASSERT 没写
  `context: :contexts`，整份计划被拒，回执失败。
- recall 的 `max_output_tokens` 按 UTF-8 字节数保守限制（每个 o200k token 至少一个字节，所以
  不会少算，可能比 Rust 服务更早裁掉可选条目），装得下的最多可选条目用二分查找确定。
  展开默认预算为 65,536。`budget.deadline_ms` 同时是本次 Recall 模型调用的截止时间：超时即取消
  模型调用，briefing 以 `the recall pass did not complete` 不确定性返回，而不是让调用在后台继续。
- 省略 `observed_at` 的暂存重试复用首次观察时间；feedback 保留请求作用域。
  误记失效后，依赖它的模型摘要会被替换。来源遗忘包含有新建轨迹的记忆概念；
  轨迹缺失或处理未完成则报告 partial，不删除共享人物/选项概念。
- 作用域句柄（`memory_scope:<handle>` 键的 Event）按键解析，不限生命周期状态（已擦除除外）：
  维护计划选中 Event 时即使归档或合并了句柄，作用域内的记忆仍可召回，新的观察也继续写入该作用域。
- forget 的存储表面只有本 Durable Object：图元素、Evidence 载荷与暂存来源字节；没有会话记录
  需要清理。

## 确定性 settlement 与受保护操作

每次 maintenance 在模型调用前执行更正发现和结构化 Watch 推进。没有衰减扫盘：强度在
读取时由基值、锚点与钉住的 `strength_policy` 计算，缺失即未知，绝不衰减 Assertion
confidence。读取不强化记忆。`parameters.memory_strength_decay_factor` 已弃用，仍做范围
校验但不生效。宿主在每条模型命令里绑定 `:strength_policy`（标准 `kip:strength-half-life-30d`）
与 `:now`；模型写 `memory_strength` 时必须同时写 `last_metabolized_at` 与 `strength_policy`，
否则闸门拒绝。到期、状态为 `pending`/`blocked` 且没有 Watch 的 Commitment 由 settlement
原生写一条 `commitment_review` Activity 提起为注意力，键为 `commitment_review:<id>:<due_at>`：
重放不再提起，改期后才再次提起（报告字段 `commitments`）。
更正扫描通过 `(space_seq, assertion_id)` 继续分页，同一事务超过 20 条也不会丢弃尾部。
响应中的 `settlement.corrections.incomplete` 表示尚未证明 backlog 已读完；`cursor_after_id`
在需要从事务内部继续时出现。发现进度不表示模型已处理这些更正。

Watch 先以 `disarmed` 创建，再通过宿主 `armWatch` 领取新 generation 与授权观察依据。
结算调用 `advanceWatch`，提交整体版本与 generation，让 Nexus 检查完整性水位和权限变化。
只有完整授权覆盖越过截止时间，silence Watch 才能触发；等到的变化已经发生时，截止后
状态为 `expired`，兼容响应字段 `disarmed` 统计该数量。引擎返回状态、coverage、receipt，
不会凭空生成 `watch_fire` Activity。文本条件和混合 text/selector 条件保持 deferred；
本适配器没有语义 evaluator。模型完成一轮不再被记录成 `consumed_seq`。旧 Watch 缺少
WatchState 时，先审查观察缺口，再显式重新 arm。

Skill 保持稳定身份，行为放入不可变 SkillRevision，current_revision/revision_of 双向引用
可在同一 MUTATE 创建。旧的 family 成功率规则已删除：family 仅用于寻找可比较样本，
不能自动选定基线。未配置独立观察者、冻结 TrialRecord、重放材料和受保护评估策略时，
程序候选保持未验证，`settlement.skills.unsupported_reason` 说明未运行评估；旧计数
字段仍为零。模型不能写学习记录、WatchState、LeaseState、Skill 的 `current_trial` /
`current_evaluation`，也不能写计算得出的 GradingState 与血缘字段。

模型计划可选两个宿主字段（普通 HTTP 请求形状不变）：

```json
{
  "types": [], "predicates": [], "commands": [], "summary": "领取维护任务，下一轮处理。",
  "digests": {"digest_revision": {"task_family":"deploy", "procedure":"verify first"}},
  "runtime": [{"operation":"lease_task", "target_ref":"C-12", "expected_version":3}]
}
```

`digests` 至多四项、总量至多 64 KiB，键以 `digest_` 开头；宿主以 kip-jcs-safe-v1
规范化 JSON 并计算 SHA-256，作为 `:digest_revision` 等参数传给 KML。revision 摘要覆盖
全部 attributes，排除 behavior_digest。Nexus 会复核摘要与实际行为一致。

`runtime` 至多四项，支持 `arm_watch`、`lease_task`，仅 Maintenance 可用。顺序为：
校验整份计划 → 发布合法词汇 → 执行 runtime → sequence/stop 执行 KML。租期由宿主固定
为五分钟；身份来自认证 Session。每次操作后需要重新读取整体版本，这个单轮模型应在
下一次 snapshot 中读取结果，再把任务终态与输出放进同一个有 CAS 的 MUTATE。
runtime 结果通过 maintenance 响应的 `runtime` 数组返回；后续失败不会抹掉先前操作的
receipt，错误数据保留已执行的结果。整个计划不是事务，不要自动重放已成功的前缀。

使用 CognitiveMemory 2.0 精确 `schema_ref` 写入的旧 Watch/SleepTask 不能原地获得
2.1 的 WatchState/LeaseState。runtime 会在调用受保护操作前返回迁移说明：创建 2.1
替代记录，复制并核验必要的语义字段和结构引用，再归档旧记录；不要复用按 lineage
唯一的旧 key。

`assessment.revised_roots` 仍提供有界依赖遍历；缺页或不可访问的闭包显式标记 incomplete。
Nexus 的虚拟 dependency_validity 决定派生内容能否使用，存储的 review 不能覆盖它。
快照里的 space_seq 也不等于 WorkingState 的真实计算依据；缺少实际版本/basis 时推迟刷新。

## 词汇表：草稿词汇

Cognitive Memory Profile 提供标准记忆类型和谓词，业务领域仍可能需要新符号。KIP 2.0 的
Space 草稿词汇（§20.16）让 `DEFINE PREDICATE` / `DEFINE CONCEPT TYPE` 往本空间的
`kip://local/draft@0.0.0` 添加一个符号：只增不改，也不会遮蔽任何已有符号。

Formation 计划里的 `DEFINE` 命令会先于其他命令逐条执行，随后的 `MUTATE` 就能使用新符号：

```json
{
  "types": [],
  "predicates": [],
  "commands": [
    "DEFINE PREDICATE \"works_on\" {description: \"The subject works on the object.\"}",
    "MUTATE { … }"
  ],
  "summary": "记录了 Alice 在做 Aurora 项目。"
}
```

- 名称与定义体必须是字面量；类型必须 UpperCamelCase，谓词必须 snake_case，且不能是 Core 元素种类。
- 名称已能解析（`SchemaSymbolConflict`）时该命令记为 `no_effect`，计划继续执行。
- 每个空间自有符号（草稿加旧宿主包）最多 512 个。
- 每个新符号排一个 `review_schema` SleepTask（键 `review_schema:<kind>:<ref>`）。Maintenance 审阅它、在总结里提议晋升，但不能定义或晋升符号：它的 `DEFINE` 被拒绝，`types` / `predicates` 字段记为 `rejected`。
- `types` / `predicates` 字段作为已弃用的快捷方式保留一个版本：宿主用通用描述起草这些裸名称。
- `GET /v1/{space}/vocabulary` 查看草稿与旧宿主包的符号；`GET /v1/{space}/schema/drafts` 列出草稿定义及晋升去向；`POST /v1/{space}/schema/promote`（`{kind, from, to}`）由持有 API key 的所有者把草稿晋升到已安装符号，至多一次。

早先已有 `kip://anda-brain/memory` 包的空间会继续激活它（只读，不再添加符号）。Durable
Object 构造时重新激活「Profile + 该旧包」；草稿包是 Space 状态，任何激活都会保留它。

## 检索

`SEARCH CONCEPT | PROPOSITION | EVIDENCE | COGNITION` 由 `kip-do` 基于 SQLite FTS5 + BM25 实现，索引在**写入同一个事务里**维护，所以答案里的 `index_seq` 和 `current_space_seq` 相等是构造保证，不是乐观估计（§66.5、§79）。

中文分词用 `Intl.Segmenter`（ICU 词典），在进程内同步完成，读写两条路径调同一个函数。1.x 那套外部 `cf-tokenizer` 服务和 `TOKENIZER` binding 都不再需要——2.0 的写入路径是同步的，事务里发不出 HTTP 请求。

```bash
curl http://localhost:8787/v1/alice/execute_kip_readonly \
  -H 'Content-Type: application/json' \
  -d '{"command": "SEARCH CONCEPT :term WITH TYPE \"Person\" LIMIT 10", "parameters": {"term": "深色模式"}}'
```

引擎没有的三样，写了会被拒：`MODE "semantic"` / `"hybrid"`（没有嵌入模型）、`AS OF SEQ`（索引不留自身历史）、`SEARCH ASSERTION` / `SEARCH ACTIVITY`（没有自由文本可索引，返回空会被读成「没有这条主张」）。

命中是**信封**：`{id, kind, score, element}`，类型和名字在 `element` 上而不是并列。`score` 是检索相关度，不是置信度；miss 也不是「不存在」。

`recall`、`probe` 和引用列表都由一条固定的 `SEARCH CONCEPT` 打底，跑在问模型之前——答案不该取决于规划器有没有想到去查。

## 提示词

三个模式提示词是 KIP 2.0 参考 Brain 策略（`anda-db/rs/anda_kip/brain/Brain*.md`），各自附一段本部署的契约（`# A. Anda Brain Worker deployment contract`），每次调用（包括 Recall 回答与 Formation 复核）再拼上完整 KIP 语法、对应角色卡与 Cognitive Memory Profile。它们放在 `assets/`，由脚本内联：

```bash
pnpm run sync:assets      # 从 anda_kip 刷新全部 vendor 资产
pnpm run codegen:prompts  # assets/*.md -> src/assets.generated.ts（需提交）
```

`sync:assets`（仓库根的 `scripts/sync-kip-assets.mjs`，Rust 侧的 `anda_brain/assets/` 也归它管）逐字覆盖语法卡和 Profile，并且**按 `# A.` 标题切开**每份 `Brain*.md`：上半截参考策略从 `anda_kip` 重刷，下半截本部署契约原样留下。之前这三份写着「上游变了就手工 diff」，结果 KIP 2.0 `40e655f` 的 Watch、WorkingState、DerivationState、`MnemonicState.utility`、`LIST DEPENDENTS`、`PURGE PAYLOAD` 在参考策略里躺了好几天，五份副本一份都没有。

Rust 服务不需要 codegen：`anda_kip` 随协议发出语法卡和 Profile，运行时直接读。`kip-do` 两者都不发，所以这里保留副本——代价就是副本会漂移，`sync:assets` 是用来对抗这件事的。

提示词包含角色策略和完整 ontology；实际 token 成本应按部署模型的 tokenizer 测量。`AI_MODEL` 默认 `@cf/meta/llama-4-scout-17b-16e-instruct`；生产环境可换成上下文更大、同样支持结构化 JSON 输出的模型。

### 内嵌参考查阅

提示词中的 Markdown 相对链接只表示出处，不是运行时可打开的文件。Worker
内嵌 21 份版本锁定的协议参考，包括规范、语法、角色卡、Profile、EBNF 和 JSON
Schema；部署后读取它们无需源码目录或网络。参考说明不代表 Worker 支持协议的全部能力。

Worker 使用结构化 JSON 中的可选 `references` 字段完成只读查阅，不要求模型支持
原生工具调用。例如 Recall 规划阶段可以先返回：

```json
{"commands": [], "references": [{"document": "syntax", "section": "kql", "offset": 0}]}
```

`document="index"` 列出文档 ID；`section="index"` 列出精确章节名，`section=null`
读取全文；语法卡另有 `kql/kml/meta/envelope` 别名。正文每页最多 8 KiB UTF-8 字节，
使用返回的 `next_offset` 续读同一文档/章节。未知 ID、章节和非法偏移会返回错误。
每阶段最多 3 轮查阅、8 页，每轮最多 4 页；达到限制后必须提交最终结果。
不查阅时保留原来的单次调用路径，查阅产生的所有模型调用计入 `usage`。

Formation、Maintenance、Recall 规划及回答阶段均支持查阅。查阅响应中其他必填字段
必须使用空占位值：计划的 `types/predicates/commands` 为 `[]`、`summary` 为 `""`；
回答的 `answer=""`、`found=false`、`uncertainty=1`。禁止夹带执行计划、词汇声明、
digest 或运行时动作。最终结果省略 `references` 或使用 `[]`，才会进入原有校验及执行流程。
查阅不会读取图谱、更新快照、扩展权限，也不构成记忆证据或变更覆盖。

`@ldclabs/kip-do` 使用 npm 上的 0.14；协议参考跟随根目录 `Cargo.lock` 解析出的 `anda_kip`（0.14.2），codegen 会校验两者一致。
生成检查验证协议 pin、各参考文件 SHA-256 与生成文件；引擎补丁版本不改变协议资料来源。
从仓库根目录刷新资源：

```bash
ANDA_KIP_SOURCE=/path/to/anda_kip-0.14.2 node scripts/sync-kip-reference.mjs --worker
pnpm --filter @ldclabs/anda-brain-worker run codegen:prompts
```

`assets/kip-reference.json` 保存补充正文与 SHA-256 清单，已有角色卡、语法和 Profile
复用原来的资产。codegen 校验版本和哈希后生成 `src/references.generated.ts`。
`pnpm check` 同时检查生成文件漂移、类型、测试和部署打包；上游参考正文不得手改。

## 快速开始

从仓库根目录安装并启动：

```bash
pnpm install
cp anda-brain-worker/.dev.vars.example anda-brain-worker/.dev.vars
pnpm --filter @ldclabs/anda-brain-worker dev
```

本地调试可把 `.dev.vars` 中的 `BRAIN_API_KEY` 留空。部署前应配置密钥：

```bash
cd anda-brain-worker
pnpm wrangler secret put BRAIN_API_KEY
pnpm run deploy
```

## API

当 `BRAIN_API_KEY` 非空时，所有 `/v1/{space_id}/*` 请求都必须带：

```text
Authorization: Bearer <BRAIN_API_KEY>
```

### Formation

```bash
curl http://localhost:8787/v1/alice/formation \
  -H 'Content-Type: application/json' \
  -H 'Authorization: Bearer replace-me' \
  -d '{
    "messages": [
      {"role": "user", "content": "以后回答尽量简洁，并优先给结论。"}
    ],
    "context": {"counterparty": "alice", "source": "chat-42"},
    "timestamp": "2026-08-20T12:00:00.000Z"
  }'
```

Formation / Maintenance 的 `timestamp` 接受任意时区偏移、最多毫秒精度的 RFC 3339，
宿主统一转为 `YYYY-MM-DDTHH:mm:ss.SSSZ`；无法解析或精度超过毫秒时返回 400，不会被悄悄
替换。缺省时使用本次请求的接收时间。消息自带的 `timestamp`（Unix 毫秒）是该条消息的
观察时间；规划上下文的 `captured_evidence` 列出每个 `:msgN` 的观察时间，模型据此写主张的 `at`。

`context.counterparty` 是 Concept 的 **key**（不可变身份），不是 name（可变标签）。宿主会在规划前确保该 Person 存在，并保留已有显示名称。

`context.source` 是线程/渠道来源，不是消息去重键。Evidence 身份由完整输入、上下文和时间戳的摘要确定；
同一 source 的后续消息不会复用旧消息的 Evidence。要重试同一观察，应保持消息、上下文及显式 `timestamp`
不变；省略 timestamp 时，每个请求获得新的观察时间。此规则只保证 Evidence 的身份，不表示整份模型写入计划
具备请求级 exactly-once 语义。

Formation 只能写认知：`CREATE CONCEPT`、`UPSERT CONCEPT`、`ENSURE PROPOSITION`、`CREATE EVIDENCE / ASSERTION / ACTIVITY`、`ASSERT`，以及用于更正和自身 Activity 的 `TRANSITION`——状态限于 `retracted` / `superseded` / `corrected` / `running` / `completed` / `failed` / `cancelled`。`TRANSITION ... TO "archived"`、`TO "tombstoned"` 以及 `UPDATE`、`PURGE`、`MERGE CONCEPT` 会在 Durable Object 内被拒绝。状态必须写成字面量：闸门读不到的参数化状态一律拒绝，否则「六条语句合并成一条 `TRANSITION`」就等于给 Formation 开了一条以绑定值 tombstone 的路。

输入 JSON 达到约 40,000 字符（10K token 的成本估算阈值）时，初次写入成功后追加一次
Formation 语义复核。复核保留原始有界来源、每条实际回执和最近 16 条消息的精确 Evidence
绑定窗口；至多执行一个最小修复命令，不重新编码整份输入，也不扩大 Formation 权限。
返回 `review` 与累计 `usage`；若复核失败，422 错误包含初次写入/修复回执及 usage，
不能把它当作初次写入已回滚并重试整份输入。该检查不保证完整语义覆盖。

### Recall

```bash
curl http://localhost:8787/v1/alice/recall_structured \
  -H 'Content-Type: application/json' \
  -H 'Authorization: Bearer replace-me' \
  -d '{
    "query": "我应该用什么风格回答 Alice？",
    "context": {"counterparty": "alice"}
  }'
```

召回计划只能执行 KQL / META，且每条必须带 `LIMIT 20` 或更小；无界、可变更或无法解析的命令会被静默丢弃。服务在首次模型规划调用**之前**固定运行 grounding 查询，并把结果交给规划器，因此模型给出无效计划时仍能完成基本召回——它失败才算服务失败，模型规划的那几条失败只会记进 `diagnostics.planned_read_errors`。

Worker 尚未实现 Rust 的 tokenizer 计数 Recall packet 和累计规划输入预算；非空 `budget`
明确返回 400，不能降级成无预算回答。提示词按字符的截断不是 token 预算。

### Maintenance

```bash
curl http://localhost:8787/v1/alice/maintenance \
  -H 'Content-Type: application/json' \
  -H 'Authorization: Bearer replace-me' \
  -d '{"trigger":"on_demand","scope":"daydream"}'
```

Maintenance 可以使用受限的维护 KML，学习和运行时 Facet 由宿主保护； **`PURGE` 和 `PURGE PAYLOAD` 被拒绝**（不可逆，且模型读自己的快照不是决定「让某物从未存在」的地方；需要时走管理级 `execute_kip`）。**任何带 `WHERE` 选择的子句都必须带 `LIMIT 20` 或更小**——`UPDATE ?e … WHERE {}` 和 `TRANSITION ?e TO "archived" WHERE {}` 是同一个风险换了个动词。`MERGE CONCEPT` 语法上没有 `LIMIT` 位置，所以对它的要求是 `WHERE` 必须精确指出源和目标，一次合并一对。

`SET RETENTION` 引擎已实现，maintenance 可以写保留期类别和 `expires_at`；被拒的只有 `legal_hold` 这一个成员，两个方向都拒——法务保留会挡住所有人的擦除，不是模型读一张快照就该做的决定。到期清扫本身 Worker 没有，写下的 `expires_at` 要靠调用方或 Rust 服务去执行。

请求参数：`stale_event_threshold_days`、`unconsolidated_max_backlog`（兼容旧名 `unsorted_max_backlog`）、`orphan_max_count`——与 Rust 服务的 `MaintenanceParameters` 逐字段对齐；已弃用的 `memory_strength_decay_factor` 仍被接受并校验 (0, 1]，但不生效。没有 `confidence_decay_factor`：2.0 禁止随时间衰减 Assertion 置信度。

### 注意力召回

`GET /v1/{space}/memory/attention?attention_cursor=attention:41&limit=20` 返回游标之后
已触发的 Watch（`watch_fired`）与到期的 Commitment（`commitment_due`），按
`(raised_seq, ref)` 排序（`raised_seq` 是提起它们的 `watch_fire` / `commitment_review`
Activity 的 `space_seq`），并返回新的 `attention_cursor`。一次提交可以提起多个条目，页面
可以停在它们中间（游标形如 `attention:<seq>:<ref>`），下一页从最后交付的条目之后继续。
游标由调用方在取走条目后保存，不会过期；缺省、`attention:start` 或旧的 `attention:-1`
表示从头读起。读取不改变记忆，条目不授予任何权限。与 Rust 服务的同名接口形状一致。

### 显式图谱擦除

`POST /v1/{space}/memory/forget` 接受 `{"entities":["E-4","X-5"],"dry_run":false}`，
单次最多 100 个元素 id，支持 `C-`、`P-`、`A-`、`E-`、`X-`。输入去重，逐目标调用
原生 `PURGE … REFERENCE POLICY "authorized_cascade"`，保留 legal hold 和权限检查。
`dry_run:true` 只检查存在性，不删除，也不保证实际擦除一定通过。响应包含
`deleted_concepts/propositions/assertions/evidence/activities` 实际计数与每个目标的
`existed/error`；一个目标失败不掩盖其他目标的成功。
已归档元素也可按 ID 清除；`existed:false` 不会将仍在存储中的归档记录误报为不存在。

该管理 API 会使旧模型处理代次失效，并清除被擦除记录在产品预览中的副本，但不封禁新输入
来源。需要可复核的来源封禁和可恢复闭包删除时，使用受信宿主的
[productPrepare / productCommit](PRODUCT_cn.md)。这些产品方法不暴露为 HTTP 或模型工具。

### Probe 与直接 KIP

| Method | Path | 说明 |
| --- | --- | --- |
| `GET` | `/healthz` | 健康检查，不需要鉴权 |
| `GET` | `/v1/{space}/info` | 活跃元素计数（不含已归档与已擦除的存根）、Schema 环境版本和初始化时间 |
| `GET` | `/v1/{space}/vocabulary` | 本空间的草稿符号与旧宿主包符号 |
| `GET` | `/v1/{space}/schema/drafts` | 草稿定义及晋升去向 |
| `POST` | `/v1/{space}/schema/promote` | 所有者把草稿晋升到已安装符号 |
| `GET` | `/v1/{space}/formation_status` | 同步模式状态 |
| `POST` | `/v1/{space}/memory/forget` | 显式图谱擦除与逐种类计数 |
| `POST` | `/v1/{space}/probe` | 不调用 LLM 的轻量记忆查找 |
| `POST` | `/v1/{space}/execute_kip_readonly` | 只允许 KQL / META |
| `POST` | `/v1/{space}/execute_kip` | 管理级原始 KIP，允许写入 |

直接 KIP 的请求体与 Rust 服务一致：`{"command": "..."}` 或 `{"operations": [...]}`，二选一。`parameters` 会绑定进命令的 `:placeholder`（结构化绑定，不是字符串插值），单个 operation 自己的 `parameters` 覆盖共享的同名键。每个 operation 可以带自己的 `op_id`，会原样回显在对应结果上——这是批次答案与请求配对的唯一可靠方式。

`execution` 可选：`{"mode": "independent"}`（默认，各自独立提交）或 `{"mode": "sequence", "on_error": "stop"}`（一条失败后，其余答 `skipped` 而不执行）。`"atomic"` 被明确拒绝——本引擎没有跨 operation 的事务，把它当 sequence 跑就等于谎报了原子性。

含 `PURGE` 或 `PURGE PAYLOAD` 的 `execute_kip` 批次与 `memory/forget` 一样：执行前开启新的处理代次，
进行中的 Formation / Recall / Maintenance 以 409 结束并需重建上下文；执行后从产品预览副本清除被擦除的
元素。托管变更尚未完成时这样的批次返回 409（`memory_change_pending`）。

```bash
curl http://localhost:8787/v1/alice/execute_kip_readonly \
  -H 'Content-Type: application/json' \
  -d '{
    "operations": [
      {"command": "FIND(?c.id, ?c.name) WHERE { ?c CONCEPT {type: \"Person\", key: :who} } LIMIT 5"}
    ],
    "parameters": {"who": "alice"}
  }'
```

响应是每个 operation 一项的数组。每项都带必填的 `status`（`succeeded` / `failed` / `skipped` / `no_effect`），读操作的行在 `result`，写操作的完整事务结果（`handles`、`changes`、治理决定）在 `extensions["kip-do/outcome"]`，失败在 `error`。判断成败请读 `status`：`no_effect` 既没有 `error` 也没有提交，把「没有错误」当成「写进去了」会漏掉这种情况。

## 安全边界

- API Key 为空会关闭鉴权，只适合本地开发。
- 只读、Formation、Maintenance 三道闸门都按命令**解析出来的语义**判断，而不是请求里的标签；Worker 和 Durable Object 两层都会校验。
  只读那条还有第三层：Durable Object 把引擎自己的 `readonly` 标志一并传下去，由语句路径内部再判一次 `parseKip` 的结论。闸门是给出答案的那个——它在任何东西跑之前就点名拒绝的命令；引擎标志是底板，上面漏了只会赔上一个 `ReadonlyViolation` 的 operation，而不是一次已提交的写入。
- 模型看到的对话、查询和图谱内容都被标记为数据，不能改变系统规则。
- 原始 `execute_kip` 是管理接口；不要把密钥交给不可信客户端。
- 一个空间对应一个 Durable Object，KIP 存储操作会在其中串行提交；AI 规划仍可能并行运行，高吞吐场景应拆分空间。

## 处理可靠性与返回状态

- 同一 Space 的 Maintenance 为单次运行；重叠请求返回 HTTP 409，
  `error.data.code=maintenance_busy`。运行标记持久化，过期接管使用新身份，旧计划不能写入。
- 更正扫描保留未确认页。Maintenance 计划可返回 `reviewed_corrections:["A-1"]`，
  仅确认本次快照中实际审阅的根；失败、延后或省略确认时，下轮仍提供这些根。
  确认有界审阅不证明依赖闭包完整，也不覆盖原生 dependency validity。
- 快照先在 SQLite 选每组最多 20 个候选 ID，再通过原生授权读取完整元素视图与版本，
  并提供 live primer。任务排除终态，Memory Interface 的作用域句柄 Event 是宿主记账，不进入快照；候选轮转位置表示已提供，不表示已处理。
  KIP 查询的 `LIMIT` 仍是结果条数上限，不是通用扫描工作量上限。
- Formation / Maintenance 返回 `operation_results`，每项包含 `status` 和可用的原生
  `receipt`（以及请求指定时的 `op_id`）。没有实际计划变更时，宿主返回无变更文案，
  不直接复述模型的成功摘要。Maintenance 的原生 settlement 仍单独报告。
- `usage.input_tokens/output_tokens` 为数值或 `null`；`null` 表示至少一个调用缺少该项
  计量。此时 `usage.known` 保留已知部分之和，不能当作整个请求的实际总量。
- `AI_TIMEOUT_MS` 是所有模型阶段共享的截止时间，默认 120000，允许 1–300000 毫秒，
  包括参考查阅、Recall 回答和 Formation 复核。宿主传递取消信号并丢弃迟到输出。
  模型超时返回 HTTP 504（`model_timeout`）；其他模型调用/输出错误返回 HTTP 502。
  错误数据保留已知用量。初次写入后复核失败仍返回带回执的 422，不能重放整份计划。
- 管理修改后的模型 Session 在原生读取时排除非活跃元素，覆盖结构引用、按 ID 读取
  Proposition、嵌套查询与聚合；管理审计接口仍可读取历史。模型读取的能力因此以 Space 的
  第一次托管变更（产品变更、forget 或管理级擦除）为界：此前 Recall 规划可用普通只读 KQL / META，
  此后只接受当前 KQL 与 SEARCH，历史读取、continuation cursor 和其他 META 被拒绝。HTTP 409 的
  `error.data.code` 给出精确处理原因，模型供应商错误文本不会被子串匹配成状态冲突。
- 批量擦除合并预览清理，分批读取历史预览并清除过期草稿内容；清理失败保留恢复记录，
  后续对象访问/驱逐恢复会继续清理，完成前自动处理保持关闭。

## 检查

```bash
pnpm --filter @ldclabs/anda-brain-worker typecheck
pnpm --filter @ldclabs/anda-brain-worker test
pnpm --filter @ldclabs/anda-brain-worker deploy:dry-run
```

`pnpm --filter @ldclabs/anda-brain-worker check` 三条一起跑。测试运行在 workerd 中，会实际覆盖 SQLite Durable Object、KIP 2.0 写入、词汇表发布与驱逐后恢复、中英文 SEARCH 落地、只读边界和召回链路。
