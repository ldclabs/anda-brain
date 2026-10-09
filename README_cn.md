# 🧠 Anda Brain (大脑) — 为 AI 智能体打造的长期图谱记忆

> 消耗电力训练大模型，得到神经网络本体；消耗词元训练记忆图谱，得到符号网络本体。
>
> 两者结合，就是**神经符号 AI**——而 Brain 正是那颗让符号网络持续生长的认知器官。

**[English](./README.md) | [中文](./README_cn.md)**

Anda Brain 是一个自托管的 LLM 智能体记忆服务。智能体把观察到的内容交给它，用自然语言向它
提问；Brain 把这些交互整理成一张有版本的知识图谱——存放在
[AndaDB](https://github.com/ldclabs/anda-db) 中的**认知中枢（Cognitive Nexus）**——并通过
后台的“睡眠”周期维护这张图谱。内部一切都用 [KIP 2.0](https://github.com/ldclabs/KIP)
（Knowledge Interaction Protocol，知识交互协议）表达，但业务智能体完全不需要编写 KIP。

- **Formation（记忆编码）**：把对话编码为结构化记忆。
- **Recall（记忆召回）**：用记忆回答自然语言问题。
- **Maintenance（记忆维护）**：在空闲时整合、复核并退役记忆。

当前版本：**0.13.4**，对齐 KIP `11a82ec` 与 Cognitive Memory Profile
`kip://profiles/cognitive-memory@2.0.0`。变更见 [CHANGELOG](./CHANGELOG.md)。

---

## 为什么还需要一种记忆系统？

你的助手记得你说过的每一句话。可当你请它推荐餐厅时，它兴高采烈地推荐了一家巴西烤肉——
尽管你上个月刚告诉它你开始吃素了。

这不是检索失败。它既检索到了两年前的“我爱吃烧烤”，也检索到了上个月的“我现在吃素”，只是
无从知道后者取代了前者：在它的存储里，这是两个平等的点，没有时间线、没有来源，也没有关系。

| 方案 | 问题出在哪里 |
| :--- | :--- |
| **向量 RAG** | 片段是彼此独立的点。没有任何东西表明其中两个描述的是同一个人的同一项偏好，也无法表明一个终结了另一个。 |
| **Markdown 备忘录** | 每次整理都要重读整个文件。文件越长，每一轮越贵、越不准确。 |
| **键值存储** | `alice.diet = "omnivore"` 覆盖了 `"vegetarian"`，历史随之消失。 |
| **图数据库 + LLM 写查询** | 数据结构是对的，但让模型针对僵硬的 Schema 写 Cypher 容易出错，集成也很重。 |

记忆真正需要的操作——合并同一主题的碎片、发现变化、保留时间线、权衡证据——都是对网络的
操作。Anda Brain 用图来保存记忆，并把维护图谱的工作交给专门的智能体，你的业务智能体不必操心。

---

## 工作原理

### 三层架构

```
┌──────────────────────────────────────────┐
│   客服智能体 · 销售智能体 · 研发智能体   │  ← 业务智能体
│   自然语言、REST 或 MCP                  │    无需了解图谱或 KIP
└────────────────┬─────────────────────────┘
                 │ Formation / Recall / Memory Interface
                 ▼
┌──────────────────────────────────────────┐
│               Anda Brain                 │  ← Formation · Recall · Maintenance 智能体
│   唯一使用 KIP 的一层                    │    以及宿主闸门、结算与调度
└────────────────┬─────────────────────────┘
                 │ KIP 2.0（KQL / KML / META）
                 ▼
┌──────────────────────────────────────────┐
│   基于 AndaDB 的 Cognitive Nexus         │  ← 持久、有版本、可审计的图谱
│   Concept · Proposition · Assertion      │    每个记忆空间一个数据库
│   Evidence · Activity                    │
└──────────────────────────────────────────┘
```

每个 **Space（记忆空间）** 都是一份隔离的记忆：独立的数据库、图谱、对话历史、令牌和策略。
一个 Brain 实例可以服务多个 Space，多个智能体也可以共享同一个 Space——客服智能体学到的，
销售智能体就能召回。

### 三个智能体

| 智能体 | 做什么 | 类比大脑 |
| :--- | :--- | :--- |
| **Formation** | 读取一段对话，对照已有记忆落地实体，写下值得保留的内容：Evidence、主张、事件、经验、承诺。异步执行，并按顺序处理一个 Space 的队列。 | 编码新经历 |
| **Recall** | 为问题规划只读的图谱查询，沿关系多跳追踪，并只按记忆真正支持的内容作答——包括“有争议”和“依据不足”。 | 回忆 |
| **Maintenance** | 把事件整合成知识，复核身份与矛盾，重新检查依赖于已修正主张的内容，复核保留期、承诺和新词汇。 | 睡眠 |

### 记忆长什么样（KIP 2.0）

KIP 2.0 把含义、信念、证据和来源分开保存。其余一切都源于一条规则：**一个陈述存在，不等于
它为真。**

| 元素 | 含义 |
| :--- | :--- |
| **Concept** | 可被引用的事物：一个人、一个项目、一个选项、一次事件。 |
| **Proposition** | 真值中立的 `(主语, 谓词, 宾语)` 陈述。 |
| **Assertion** | 某个行动者对 Proposition 的立场：谁说的、多大把握、从何时起、引用了什么。 |
| **Evidence** | 观察到的内容：一条消息、一个工具结果、一段文档。 |
| **Activity** | 某样东西如何产生：一次编码、一次整合、一次修订。 |

“当前相信什么”在读取时由 Assertion 推导得出，而不是存储下来的。这让 Brain 具备了一些用
其他方式很难得到的性质：

- **分歧可以共存。** “Alice 说 X，Bob 说非 X”仍然是两条 Assertion，不会被悄悄裁定输赢。
- **三种变化，三种历史。**
  - *世界变了*（“我现在吃素”）：从变化时刻起记一条新 Assertion，由时序继承结束旧值，旧值
    仍回答它所在时段的问题。
  - *主张错了*：同一行动者的新 Assertion 取代（supersede）旧的那条。
  - *Brain 记错了*：记录修复使错误的抽取失效，任何人的立场都不变。
- **每条主张都可追溯**到行动者、Evidence 和观察时间。主张的 `asserted_at` 是其来源被观察到
  的时间，而不是 Formation 运行的时间。
- **遗忘关乎可及性，从不关乎真假。** 记忆强度随闲置衰减，并在读取时计算；Assertion 的置信度
  永不衰减。一个月没人问起的事实，并不因此变得不可信。
- **读取不会强化。** Recall 是只读的；只有明确的信号（某个决策用到了某条记忆、一次更正）才会
  改变强度。
- **“不知道”不等于“不是”。** 依据不足就报告依据不足。
- **新词汇先是草稿。** 当 Formation 需要 Profile 中没有的类型或谓词时，会把它起草到 Space 自己
  的草稿包中。宿主校验命名、为每个 Space 设 512 个符号的上限，并排队复核；只有 Space 所有者
  才能把草稿晋升到已安装的符号上。

### 睡眠：维护周期

Maintenance 有三种深度：

| 范围 | 触发 | 工作 |
| :--- | :--- | :--- |
| `daydream` | 每编码 21 段对话，或按需（默认） | 显著性评分和近期材料的微整合。 |
| `quick` | 每编码 42 段对话 | 评估加紧急 SleepTask。 |
| `full` | 每编码 168 段对话；只要 Space 编码过内容，至少每 24 小时一次 | 全部工作，包括谓词普查和保留期到期处理。 |

每个周期都从确定性的 **settlement（结算）** 开始——由宿主而不是模型记录新的更正、遍历依赖于
已修订主张的内容、提起到期的承诺、归档超过保留期的内容，并把一份事实性的 `assessment` 交给
模型。随后 Maintenance 智能体完成认知工作：

- **整合**：一组事件和经验变成派生主张，并保留回到来源的谱系（“在三次对话里提到三文鱼、
  海胆和寿司” → “偏好日本料理”）。
- **身份**：复核疑似重复的实体，并以非破坏方式合并。
- **修订**：依赖于已被更正前提的主张被标记为过时。
- **程序**：反复的成功与失败可以编译成 Skill 候选，在独立的试验流程给出结论之前，它始终
  保持*未验证*。
- **承诺、Watch、保留期与新词汇**都会被复核。
- **自检**：周期结束后，Brain 会探测最近从未被问起的记忆，搜索找不到的就排队重新编码。

Maintenance 从不清除记忆、从不定义词汇、也从不给 Skill 评级；这些都属于明确授权的路径。

---

## 功能

**接入**

- **REST API**，请求和响应体支持 JSON、CBOR 或 Markdown。
- **KIP Memory Interface**（`memory_basic`）：一个端点、五种意图——`observe`、`recall`、
  `revise`、`feedback`、`forget`——基于暂存的来源，提供幂等回执、`after` 屏障和有作用域的
  召回简报。
- **MCP 服务**，支持 Streamable HTTP（`/mcp/{space_id}`）和 stdio，提供相同的记忆、注意力与
  Wiki 工具。
- **[SKILL.md](./skills/anda-brain/SKILL.md)** 供智能体框架接入，每个部署也会在 `/SKILL.md`
  提供它。
- **[anda-cli](./anda-cli/README.md)**：命令行客户端，覆盖密钥、令牌、Formation（含批量文件
  导入）、Recall 和 Memory Interface。

**记忆**

- 自然语言 Formation 与 Recall；带引用和 `found` 标记的结构化 Recall；可选的 Recall 结果
  硬性词元**预算**。
- 不调用模型的 **probe**（“我对这件事知道些什么吗？”），带否定结果缓存。
- **注意力召回**：已触发的 Watch 和到期的 Commitment，有序并按游标分页。
- **固定（pin）**、支持演练的显式**遗忘**，以及受治理的擦除计划。
- 用于高级检查的只读 **KIP 端点**。
- **记忆可观测性**：使用、探测、自检与更正计数，图谱统计，以及最近一次结算报告。

**运维**

- 多 Space 隔离、Ed25519 签名的 CWT 认证、Space 级令牌（`read` / `write` / `*`，可选 Wiki
  标签限制）、管理者和分级。
- 存储支持本地文件系统、AWS S3，开发时也可使用内存。
- Space 级 `MemoryPolicy` 和 Space 级 BYOK 模型配置。
- 请求削峰、进程级模型并发上限、后台落盘与空闲 Space 驱逐、优雅停机。
- 由 KIP 1.x 版本写入的 Space 自动升级。

**可选能力**（编译特性或显式运行时绑定，默认关闭）

- **Wiki**：有版本的 Markdown 参考文档，按 ACL 限定读取，可验证引用，OKF 导入导出，以及
  可选的图谱抽取（WikiDigest）。
- **运行时 API**：经过认证的注意力收件箱、响应与独立结果；跨 Space 的结构化 Watch 调度；
  带栅栏分派的动作回调。
- **语义 Watch**、**受信学习试验**、**记忆效用**和**情境来源信任**运行时，各有独立指南和默认
  关闭的配置。
- **实验**：用于评估的隔离宿主运行、快照和业务时间。

---

## 与其他方案对比

| 能力 | 向量 RAG | Markdown 备忘录 | 键值存储 | 图数据库 + LLM 查询 | **Anda Brain** |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **结构** | 文本块 | 半结构化文本 | 固定字段 | 固定图 Schema | **图谱，词汇可起草、可复核** |
| **接入** | 简单 | 简单 | 简单 | 繁重 | **自然语言、REST 或 MCP** |
| **消化** | 无 | 每轮全文重读 | 覆盖 | 很少自动化 | **定期整合周期** |
| **随时间变化** | 两个版本并存且无序 | 取决于模型 | 旧值丢失 | 自定义逻辑 | **时序继承，保留历史** |
| **错误主张** | 一直留着 | 原地修改 | 被覆盖 | 原地修改 | **由新 Assertion 取代** |
| **分歧** | 无法区分 | 取决于模型 | 后写者胜 | 自定义逻辑 | **各行动者的 Assertion 并存** |
| **来源** | 至多是来源文本块 | 无 | 无 | 视实现而定 | **每条主张都有行动者、Evidence 和时间** |
| **多跳问题** | 弱 | 弱 | 无 | 好 | **由 Recall 智能体遍历图谱** |

---

## 快速开始

> 托管服务（`brain.anda.ai`）已停止运营。Anda Brain 是
> [开源软件](https://github.com/ldclabs/anda-brain)，设计为自托管。
> 分步指南见 [deploy/quick_start_cn.md](./deploy/quick_start_cn.md)。

### 1. 运行服务

从 [Releases](https://github.com/ldclabs/anda-brain/releases) 下载 `anda_brain`，使用 Docker
镜像，或自行构建：

```bash
cargo build -p anda_brain --release --features mcp,wiki
```

```bash
docker pull ghcr.io/ldclabs/anda_brain_amd64:latest
```

选择存储后端：

```bash
# 内存存储——进程退出即全部丢失，仅用于试用
./anda_brain

# 本地文件系统
./anda_brain local --db ./data

# AWS S3（凭证来自标准的 AWS_* 环境变量）
./anda_brain aws --bucket my-bucket --region us-east-1
```

### 2. 配置模型与认证

配置可以来自命令行参数、环境变量或 `.env` 文件：

```bash
MODEL_FAMILY='anthropic'            # anthropic | openai | gemini | …
MODEL_API_BASE='https://api.deepseek.com/anthropic'
MODEL_NAME='deepseek-v4-pro'
MODEL_API_KEY='…'
ED25519_PUBKEYS='…'                 # 留空即关闭认证（仅限开发）
MANAGERS='…'                        # 允许创建 Space 的主体
```

默认值面向 DeepSeek 的 Anthropic 兼容端点。使用 Claude 模型时，请把 `MODEL_MAX_OUTPUT`
设为 128000 或更小。全部选项见[技术文档](./anda_brain/README.md#configuration)。

### 3. 创建 Space 和令牌

使用 [anda-cli](./anda-cli/README.md) 和管理者 CWT：

```bash
# 1. 生成 Ed25519 密钥对，把公钥填入 ED25519_PUBKEYS 与 MANAGERS
anda-cli keygen --json > keys.json

# 2. 用私钥签发通配管理 CWT
PRIVKEY=$(jq -r .private_key keys.json)
MANAGER_CWT=$(anda-cli cwt --key "$PRIVKEY" --subject "$OWNER" --audience '*' --scope '*')

# 3. 创建 Space 并签发智能体令牌
anda-cli --token "$MANAGER_CWT" admin create-space --user "$OWNER" --space-id my_space --tier 2
anda-cli --token "$MANAGER_CWT" --space-id my_space management add-token --scope '*' --name support_bot
```

作用域之间不存在包含关系：`*` 令牌可通过所有接口，`read` 和 `write` 令牌只能通过要求完全相同
作用域的接口。既写入又召回私有 Space 的智能体需要 `*` 令牌（签发它需要 `*` 作用域的 CWT），
或者两种作用域各一个令牌。对应的 HTTP 接口是 `POST /admin/create_space` 和
`POST /v1/{space_id}/management/add_space_token`。分级限制图谱规模：当一个第 `n` 级
Space 的 Concept 或对话数超过 10^(n+2) 时，Formation 拒绝新输入（第 0 级 100，第 2 级 10,000）。

### 4. 写入和读取记忆

把一段对话交给 Formation（立即返回，编码在后台进行）：

```bash
curl -sX POST https://brain.example.com/v1/my_space/formation \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "messages": [
      {"role": "user", "content": "我在 Acme 公司做高级工程师。"},
      {"role": "assistant", "content": "记下了：Acme 公司高级工程师。"}
    ],
    "context": {"counterparty": "user_123", "agent": "onboarding_bot"},
    "timestamp": "2026-03-09T10:30:00.000Z"
  }'
```

回复用户之前先问 Recall：

```bash
curl -sX POST https://brain.example.com/v1/my_space/recall \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "这位用户在哪里工作？", "context": {"counterparty": "user_123"}}'
```

需要回执、屏障和有作用域简报的智能体使用 Memory Interface：先暂存观察到的内容，再每次请求
发送一个意图。

```bash
# 暂存来源 → {"result": {"source_ref": "src-…", …}}
curl -sX POST https://brain.example.com/v1/my_space/memory/sources \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"messages": [{"role": "user", "content": "我上周搬到柏林了。"}],
       "observed_at": "2026-09-02T08:00:00.000Z", "idempotency_key": "chat-42:msg-7"}'

# observe → 回执从 "recorded" 推进到 "available"
curl -sX POST https://brain.example.com/v1/my_space/memory \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"kip_memory": "2.0", "operation": "observe", "idempotency_key": "observe:chat-42:msg-7",
       "input": {"source_ref": "src-…"}}'

# recall，先等待该回执
curl -sX POST https://brain.example.com/v1/my_space/memory \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"kip_memory": "2.0", "operation": "recall",
       "input": {"query": "用户现在住在哪里？", "after": ["rcpt-…"]}}'
```

### 5. 或者通过 MCP 接入

HTTP 服务会在 `https://brain.example.com/mcp/{space_id}` 挂载 Streamable HTTP MCP 端点，
用同样的令牌发送 `Authorization: Bearer …` 即可。本地客户端可以把 Brain 作为 stdio 服务运行：

```bash
MCP_AUTH_TOKEN="$SPACE_TOKEN" ./anda_brain mcp --space-id my_space local --db ./data
```

工具包括 `anda_brain_memory`、`anda_brain_stage_memory_source`、
`anda_brain_remember_conversation`、`anda_brain_recall_memory` 和 `anda_brain_wiki_search`。
完整列表见[技术文档](./anda_brain/README.md#mcp-server)。

---

## 两种部署方式

| | **Rust 服务**（`anda_brain`） | **Cloudflare Worker**（[`anda-brain-worker`](./anda-brain-worker/README.md)） |
| :--- | :--- | :--- |
| 引擎 | 基于 AndaDB 的 Cognitive Nexus | `@ldclabs/kip-do`，独立的 KIP 2.0 引擎 |
| 存储 | 本地文件系统、S3 或内存 | 每个 Space 一个 SQLite Durable Object |
| 模型 | 任意已配置的提供方，支持 Space 级 BYOK | Workers AI |
| Formation / Recall / Maintenance | ✓（Formation 在后台排队执行） | ✓（Formation 在请求内完成） |
| Memory Interface（`memory_basic`）、注意力召回 | ✓ | ✓ |
| 定期维护 | 内置 | 由调用方或 Cron Trigger 调用 |
| MCP、Wiki、预算化 Recall、CBOR/Markdown、分级令牌 | ✓ | — |
| 原子批、保留期到期清扫 | ✓ | — |
| 运行时 API、学习、效用、信任运行时 | 可选 | — |

Worker 面向小型智能体和边缘部署；它做了什么、没做什么，以其 README 为准。

---

## 使用场景

**个人智能体。** 当记忆横跨多年的人际关系、偏好和项目时，本地智能体会很快超出 Markdown
文件和 SQLite 的能力。[Anda Bot](https://github.com/ldclabs/anda-bot) 是以 Anda Brain 为长期
记忆的开源智能体。

**企业“公司大脑”。** 销售智能体记下“客户要求 Q3 前交付 5,000 台”；采购智能体随后能召回：
某核心物料的供应商过去六个月延期过三次，是谁说的、依据是什么。客服历史、决策理由和失败
教训沉淀在同一个 Space 中，每个智能体——包括新接入的智能体——都能查询。私有化部署可以让
这份记忆始终在你的掌控之中。

---

## 两种训练

| | 大模型训练 | 记忆图谱训练 |
| :--- | :--- | :--- |
| **消耗** | 电力（算力） | 词元（推理） |
| **学习对象** | 公共语料 | 你的对话与事件 |
| **产出** | 神经网络本体（权重） | 符号网络本体（图谱） |
| **赋予 AI** | 通用推理 | 身份、经历与事实 |
| **特性** | 概率、黑盒、通用 | 确定、白盒、个性化 |

模型可以替换，记忆不能。从一家模型提供方换到另一家，图谱——你的智能体积累下来的经验——
原样保留。

### 为什么叫“Brain（大脑）”？

因为它的行为就像大脑：白天**编码**经历，空闲时**巩固**知识，醒来后从更有条理的结构中
**回忆**。这个循环运转得越久，智能体对它所处的世界就了解得越多。

**是时候让你的 AI 睡一觉了。**

---

## 文档

| 文档 | 内容 |
| :--- | :--- |
| [anda_brain/README.md](./anda_brain/README.md) | 技术参考：智能体、端点、MCP 工具、配置、特性、生命周期 |
| [API_cn.md](./anda_brain/API_cn.md) · [API.md](./anda_brain/API.md) | 完整的 HTTP 与 MCP API，附 TypeScript 类型 |
| [SKILL.md](./skills/anda-brain/SKILL.md) | 面向智能体的接入说明 |
| [deploy/quick_start_cn.md](./deploy/quick_start_cn.md) · [English](./deploy/quick_start.md) | 从零到可用的 Space |
| [RUNTIME_cn.md](./anda_brain/RUNTIME_cn.md) · [English](./anda_brain/RUNTIME.md) | 运行时 API、Watch 调度、动作回调、恢复、执行限制 |
| [语义 Watch](./anda_brain/SEMANTIC_WATCH_RUNTIME_cn.md) · [学习](./anda_brain/LEARNING_RUNTIME_cn.md) · [效用](./anda_brain/UTILITY_RUNTIME_cn.md) · [信任](./anda_brain/TRUST_RUNTIME_cn.md) | 可选运行时指南 |
| [anda-cli/README.md](./anda-cli/README.md) | 命令行客户端 |
| [anda-brain-worker/README.md](./anda-brain-worker/README.md) | Cloudflare Worker 版本 |
| [anda_brain/conformance](./anda_brain/conformance/README.md) | KIP 一致性测试适配器 |
| [tools/migrate-draft-space](./tools/migrate-draft-space/README.md) | 把 KIP 2.1.0 草案 Space 迁移到新的 2.0.0 Nexus |
| [CHANGELOG.md](./CHANGELOG.md) | 版本说明与已知限制 |

## 仓库结构

```
anda_brain/              Rust 库与服务二进制
  src/agents/            Formation、Recall、Maintenance 智能体
  src/memory_interface/  KIP Memory Interface（memory_basic）
  src/space*.rs, space/  Space 生命周期、结算、自检、注意力
  src/wiki/              有版本的 Wiki（"wiki" 特性）
  assets/                智能体提示词与生成的 KIP 参考
anda-brain-worker/       Cloudflare Worker 版本
anda-cli/                Go 命令行客户端
skills/anda-brain/       打包的智能体 Skill
deploy/                  快速开始与 systemd 单元
tools/migrate-draft-space/  独立的 2.1.0 草案 → 2.0.0 迁移工具
posts/                   设计相关文章
```

## 开发

```bash
cargo fmt --check
cargo clippy -p anda_brain --all-targets --all-features -- -D warnings
RUST_MIN_STACK=16777216 cargo test -p anda_brain --all-features
CI=true pnpm --filter @ldclabs/anda-brain-worker check
```

测试使用本地存储，从不调用模型提供方。仓库约定和协议不变量见 [AGENTS.md](./AGENTS.md)。

## 状态与限制

- **KIP 版本。** KIP 1.x 的 Space 在首次打开时自动升级（请先备份；升级不可逆）。用 2.1.0 草案
  激活的 Space 不会自动迁移，请使用 [`tools/migrate-draft-space`](./tools/migrate-draft-space/README.md)。
- **每个 Space 只能有一个写入者。** 若干锁是进程内的，切勿让两个实例指向同一个 Space 的存储。
- **规模上限。** 更正发现和自检抽样使用全量扫描，引擎把结果上限定为 65,536 条；超过后它们会
  报告错误而不是完成。
- **不宣称学习带来的改进。** 除非配置了受信且有独立观察的试验流程，Skill 候选始终保持未验证。
  学习、效用和信任运行时默认关闭；它们的机制测试不能证明真实环境中的收益。

## 延伸阅读

- [AI 的记忆必须能睡眠——而只有知识图谱能让它入睡](./posts/AI_Memory_Must_Sleep_cn.md)
- [Claude Code 记忆系统深度解读：AI 是如何「记住」你的？](./posts/Claude_Code_Memory_Research_cn.md)
- [当 AI 学会本体建模：Anda Brain 让企业“长”出自己的智能大脑](./posts/Enterprise_AI_Brain_cn.md)
- [AI 的第二种训练——用词元铸造记忆图谱](./posts/Tokens_Anda_Brain_cn.md)
- [将公司构建为一个智能体，需要一颗“大脑”](./posts/Company_Built_As_Intelligence_cn.md)
- [从“编译知识”到“铸造大脑”——Anda Brain 回应 Karpathy 的 “LLM Knowledge Bases”](./posts/LLM_Knowledge_Bases_cn.md)

## 许可证

Copyright © LDC Labs。基于 Apache License 2.0 授权。
