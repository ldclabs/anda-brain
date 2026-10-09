# Rust → Worker 同步核对（2026-09-23）

> 历史快照：本文记录 2026-09-23 按当时依赖（`@ldclabs/kip-do` 0.13.2、`anda_kip` 0.13.1）做的
> 核对，其中的版本号与测试数不随后续提交更新。当前依赖是 kip-do 0.14 与 `anda_kip` 0.14.2，
> 当前能力边界以 [README](README.md) 与 [产品契约](PRODUCT_cn.md) 为准。

范围：`a6e3360^..c886b94`，包含起始提交。核对依据是提交 diff、当前两端实现和
锁定的 `@ldclabs/kip-do` 0.13.2 源码。既有 Worker 定位仍是精简的独立 KIP 引擎宿主，
不是 Rust 全产品的 API 替身。下表区分实际漏同步与原有平台差异。

| 提交 | Rust 变化 | 核对与处理 |
| --- | --- | --- |
| `a6e3360` | 时间戳归一化、Markdown Evidence、保留对方名称、Evidence/Activity 擦除 | Worker 时间戳与名称行为原已覆盖。补上 `/memory/forget` 和五种元素的实际删除计数、去重、dry run、legal hold 测试。Markdown/CBOR 提交是原有未实现传输层，本次保持 JSON。 |
| `1485520` | 每次模型调用预载完整语法、角色卡与 Profile | Formation、Maintenance、Recall 规划原已有完整语法；回答阶段确有遗漏，已补齐。复核阶段也预载完整语法。Rust 的实例提示词覆盖与实验清单没有 Worker 对应配置。 |
| `68e7fe1` | 大输入一次语义复核、保留回执与捕获窗口、只在写入附加 ingest | 补上一次复核，保留初次写入回执、原始有界来源和精确 `:msgN` 窗口；至多一个修复命令，沿用 Formation 闸门，累计全部模型调用 usage。复核失败返回初次回执，不能重放整份输入。Worker Formation 原本仅接受 KML，图读取通过独立只读 RPC，不携带 ingest。Windows 学习测试修复不适用。 |
| `1b85f32`、`c45e763` | Nexus/AndaDB 旧数据迁移与依赖修复 | Worker 使用独立 SQLite 引擎，不依赖 Rust Nexus/AndaDB。本次保留工作区已有的 `kip-do` 0.13.2 升级。没有添加旧版本迁移。 |
| `d702ed2`、`178a4e6` | Wiki 发布、版本、ACL、OKF、Digest 重试和锚点 | Worker 原本没有 Wiki，未将该子系统冒充为已移植。 |
| `2043026` | 预算 Recall 的 provider 默认输出限制、固定失败码 | Worker 没有 tokenizer 计数的 Recall packet／累计输入预算，也只有 Workers AI 适配器；其 `max_tokens` 不对应 Rust 多 provider 的拒绝行为。显式非空 `budget` 现在返回错误，不再被悄悄忽略。 |
| `ee6feb1` | 当前问题优先检索、预算候选压缩、输入不足的部分包、旧 Commitment 修复 | 普通 Worker Recall 也存在执行顺序错误：注释说先检索，代码却先规划。现已在第一次模型调用前执行查询并把结果交给规划器，引用仍按 grounding 优先。其余属于未实现的预算管线和 Rust 旧数据迁移。 |
| `544acfc` | Rust CI 与 HTTP 学习 fixture 稳定性 | 不改变 Worker 运行行为。 |
| `c886b94` | 来源记录、审阅式更正/抑制/删除、来源封禁、处理代次、恢复、记录订阅、学习就绪 | 原无对应实现。已增加受信宿主产品 RPC、当前原生权限检查、持久预览/幂等/恢复、所有模型读写与最终输出的代次检查、来源身份和父来源封禁、预览副本清理。记录 Watch 支持显式接收者绑定、原生 arming/advance、归档取消与重试不重新 arm；Worker 没有后台 inbox/执行调度。学习就绪明确返回不可用，不产生执行/观察权限。 |

产品契约详见 [中文说明](PRODUCT_cn.md) / [English](PRODUCT.md)。

## 有意保留的边界

- 协议资料仍锁定 `anda_kip = 0.13.1`；它与 TS 引擎的 `0.13.2` 是不同版本轴。
  生成检查继续验证 Cargo 中的精确协议 pin、每份参考文件的 SHA-256、全部生成产物，
  不再错误地要求 npm 引擎与协议 crate 的补丁版本相等。
- Worker 一次 `MUTATE` 有原子性，多 operation 没有。产品删除先固化最多 128 个元素
  的闭包，再逐项用版本条件擦除与检查。中断可恢复，但不会宣称是跨操作原子事务。
- 修改后，自动代理仅接受当前 KQL/SEARCH；所有 continuation cursor 都被拒绝。
  Rust 可区分修改前后 cursor；当前 TS 公共接口未提供相同的 cursor-floor 校验，
  Worker 采用更严格限制。管理级只读 KIP 仍可审计历史。
- 复核是有成本上限的语义检查，不是完整性保证；超出有界源窗口的缺失仍是未知。
  仅复核明确缺陷，不能借复核获得额外权限。
- Wiki、MCP、异步 Formation 队列、预算 packet、学习 executor/observer/source 绑定、
  语义 Watch evaluator、业务派发、HTTP inbox 与独立 Outcome 接收并非本次交付。
  它们在本次范围起点之前就不属于 Worker 的能力面。

## 验证

`CI=true pnpm --filter @ldclabs/anda-brain-worker check` 通过：7 个测试文件、89 个测试
（相对原有 Worker 新增 28 个），并通过生成资产校验、TypeScript 与部署 dry-run。
后续修复为 Rust 来源验证、直接擦除的预览清理及模型工具代次闸门加入定向回归；
`RUST_MIN_STACK=16777216 cargo test -p anda_brain --all-features` 通过 532 个库测试和
45 个二进制／集成测试。本次核对没有部署服务；v0.12.1 的版本说明见仓库 `CHANGELOG.md`。

本地 workerd / SQLite 测试覆盖原有行为，以及完整提示词、复核触发/失败/usage、
先检索后规划、预算拒绝、五类元素删除、来源链、条件预览、身份/权限撤销、
legal hold、原生回执丢失、分步删除中断、驱逐恢复、三个代理的并发代次失效，
以及记录 Watch 原生触发与取消。所有模型响应均使用确定性替身，未调用生产模型，
这些结果不构成真实模型效果、学习收益或部署校准证据。
