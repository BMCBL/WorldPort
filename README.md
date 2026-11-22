# WorldPort

WorldPort 是 BMCBL 的**世界转换工具**：在《Minecraft》基岩版（Bedrock）与 Java 版之间
双向转换存档，目标是 1:1 —— 方块状态、方块实体、实体与完整 NBT、生物群系、
`level.dat`/游戏规则、玩家、地图、tick 事件与种子。

它基于 [Chunker](https://github.com/HiveGamesOSS/Chunker)（MIT）的转换思路用 Rust
重写，但**不受 Chunker 的限制**：实体与实体 NBT 是完整转换的，而不是只转绘画/物品展示框。

## 与 Chunker 的差异（目标）

| 能力 | Chunker 1.22 | WorldPort |
| --- | --- | --- |
| 方块 / 方块状态 | 支持 | 支持（内置映射，规则随版本数据生成） |
| 含水方块 | 支持 | 支持（第二层液体 storage） |
| 方块实体 | 支持 | 支持（目标 M3） |
| 实体 | **仅绘画/物品展示框** | **全部实体 + 完整 NBT（目标 M3）** |
| 本地玩家 | 支持 | 支持（目标 M3，含背包/末影箱/效果） |
| `playerdata/` 多人玩家 | 不支持 | 支持（目标 M3） |
| 版本升降级 + 剔除 | 支持 | 支持（目标 M4） |
| 村庄兴趣点（床/工作站/钟） | 不支持 | **支持**（BE→JE 写 `poi/`，按坐标去重） |
| 村民职业/类型/等级 | 不支持 | **支持**（`SkinID`/`MarkVariant`/`TradeTier` ↔ `VillagerData`） |
| 结构生成区（要塞/神殿/前哨/女巫小屋） | 不支持 | **部分**：Java 有 piece 边界时写 Bedrock `HardcodedSpawners`；反方向只统计、不伪造 `StructureStart` |
| 结构模板（`.nbt` ↔ `.mcstructure`） | 不支持 | 未做（v1 边界） |
| tick/scheduled events | 不支持 | 支持（目标 M3/M4） |

## 项目结构

```
core/      worldport-core     转换引擎（JE Anvil + BE LayerDB）
cli/       worldport-cli      无界面工具：inspect / convert / nbt / chunk
protocol/  worldport-protocol WASM UI ↔ 原生模块的命令/事件协议
native/    worldport-native   实现 BMCBL 原生插件 ABI 的 DLL（M6）
plugin/    worldport-plugin   注册到「工具」页的 .bmcblx 插件（M6）
mappings/  版本数据 + 生成的映射表（JE blocks.json / BE block_states.json 压缩存放）
vendor/    bedrock-nbt / bedrock-leveldb / bedrock-world（来自 BMCBL crates）
sdk/       bmcbl-plugin-api / bmcbl-plugin-macros（插件 ABI 1.0.0）
```

## 快速开始（CLI）

```powershell
cargo run -p worldport-cli -- inspect "C:\path\to\world"
cargo run -p worldport-cli -- inspect --deep --limit 2000 "C:\path\to\world"
cargo run -p worldport-cli -- convert "C:\je world" "C:\out\bedrock world" --report report.json
```

调试辅助：

```powershell
# 把任意 NBT（gzip）解成 SNBT
cargo run -p worldport-cli -- nbt "<world>/level.dat"
# 打印某个区块的 SNBT / 键
cargo run -p worldport-cli -- chunk "<world>" --dimension overworld --at 0,0
cargo run -p worldport-cli -- chunk "<world>" --search --keys
```

正式性能构建使用插件/CLI 共用的 profile：

```powershell
cargo build --profile native-release -p worldport-cli
```

## 插件（工具页）

```powershell
./scripts/build-plugin.ps1
```

产物：

- `dist/worldport-0.1.0.bmcblx` —— 导入「下载 → 插件」后，「工具」页出现「世界转换」。
- `dist/native/windows-x86_64-msvc/worldport_native.dll` —— 在「设置 → 插件」批准。

插件把 DLL 的 SHA-256 烘进包内，重新构建 DLL 后要重新打包（脚本会一起做）。
页面里也能手动填写摘要（`native_sha256` 存在插件 KV，优先于烘入值）。

Wasm 沙箱没有文件系统，转换在原生组件里执行：`submit_command` 提交命令，
`read_events` 长轮询事件，取消走 `cancel_command`（越过队列立即生效）。
`native/tests/abi.rs` 直接用导出的 C ABI 跑完「创建 → 检测 → 转换 → 快照 → 销毁」。

## 当前状态

- ✅ M0：JE 大端 NBT（modified UTF-8）、Anvil 读写、经典与 26.x 分离式布局
  （`dimensions/`、`data/minecraft/{world_gen_settings,game_rules,weather,world_clocks}.dat`）、
  `inspect`/`nbt`/`chunk` 工具。
- ✅ M1：JE→BE 地形、生物群系、`level.dat`（种子/出生点/时间/天气/游戏规则）、
  v9 子区块与含水液体层；在提供的调试世界上端到端跑通。
- ✅ 映射表在**编译期**烘焙成紧凑二进制（8.2 MB JSON → 99 KB 二进制 → 15.8 KB LZ4），
  运行时一次性解压进内存，不再解析 JSON（见 `core/build.rs`）。
- ✅ M2：BE→JE 地形、群系、`level.dat`（26.x 分离式数据回写）+ `compare` 语义对比工具；
  回环实测：群系 0 差异、方块状态 96.9% 完全一致（差异集中在 Bedrock 不存储的墙连接、
  方块实体槽位等已知项）。
- ✅ 朝向映射按官方 wiki 语义 + Chunker 的 `BedrockStateTypes` 取值表校正（门旋转 90°、
  活板门 weirdo_direction、活塞/末地烛水平翻转、楼梯 corner、拉杆/中继器旧版 direction）。
- ✅ M3（部分）：物品/背包（含附魔、自定义名、药水、1.20.5 components）、方块实体
  （容器/告示牌/刷怪笼/唱片机/蜂巢等）、实体（标识符、Pos/Motion/Rotation、装备、
  状态效果、骑乘/驯服等共享字段）、玩家（`~local_player` ↔ `playerdata/<uuid>.dat`，
  含副手槽 106↔-106）。
  `worldport fixtures` 生成的测试世界回环 **方块/群系/实体/方块实体差异全部为 0**。
- ✅ M3（地图与计划刻）：地图 `map_<id>` ↔ `data/minecraft/maps/map_<id>.dat`
  （Java 调色板索引 ↔ Bedrock RGBA，64 位 id 自动重编号）；计划刻走**统一模型**
  （位置/方块或流体/剩余延迟/优先级），Bedrock `PendingTicks`（真实存档已验证
  `currentTick` + `tickList` + `blockState` + `time`）与 Java `block_ticks`/`fluid_ticks`
  双向语义映射，非原版条目计数并报告。
- ✅ 自动剔除非原版内容：非 `minecraft:` 命名空间的方块/实体/方块实体/物品在转换时
  剔除并计入报告（mod / addon 不会把未知 id 写进目标存档）。
- ✅ M4 版本识别与升降级：
  - 源版本：JE 按 `DataVersion` 选最接近的内置方块表；BE 读取 `lastOpenedWithVersion`/存储版本。
  - 目标版本：`--target-version 1.21.4`（或 DataVersion）选择目标方块表并写回对应
    `DataVersion`；布局按版本切换（1.18–1.20.x 经典 `region/`+`playerdata/`，
    1.21+ `dimensions/minecraft/<dim>/`，26.1+ 分离式 `data/minecraft/*.dat` +
    `players/data` + `data/minecraft/maps`）；物品按 1.20.5 分界写 `components` 或旧版
    `Count/tag`，实体效果写 `active_effects` 或旧版 `ActiveEffects`，地图维度写字符串或字节。
  - 降版本剔除：目标方块表里不存在的方块（如 1.21.4 的 `creaking_heart` 转 1.20.6）
    会被剔除并计入报告；`UnknownDataPolicy::{Fallback,Drop,Error}` 可切换行为。
  - 内置目标表：1.20.6 / 1.21.4 / 1.21.8 / 26.1 / 26.2（来自 misode/mcmeta，编译期烘焙；
    加载时会校验完整性，缺失基础方块的表会被忽略）；低于 1.18 的目标直接拒绝。
  - 实测：默认 26.2 回环全 0 差异；降到 1.21.4 仍全 0；降到 1.20.6 只有被剔除的
    `creaking_heart` 1 个方块差异。
- ✅ M5/M8 性能与内存（同机 16 逻辑核 / 8 物理核，`[profile.native-release]`，
  用 `./scripts/bench.ps1` 同时记录时间与峰值 RSS）：

  | 方向 | 输入 | WorldPort | Chunker 1.21.0 |
  | --- | --- | --- | --- |
  | JE→BE | 调试 JE 世界 11,192 区块 | **3.3 s / 156 MiB / 2.7 MB** | 7.0 s / 1275 MiB / 3.5 MB |
  | BE→JE | 真实 1.20.12 存档 443 MB / 108,484 区块 | **38–40 s / 276 MiB / 708 MB** | 48 s / 3943 MiB / 745 MB |

  优化过程（每项都有前后测量，负收益的改动已回退）：

  - 区块转换按 `--jobs` 并行；结果按输入顺序提交，UID 用原子计数。
  - Bedrock 侧一次 `get_many` 取齐该区块固定 tag + 24 个 subchunk（新增
    `World::chunk_snapshot`），取代逐表前缀扫描；坐标列表只扫一次 DB。
  - 并行扫描的内存按表数收敛：每个表 lane 常驻一个已解码 run，443 MB 存档有 231 张
    SST 表，原来会把 1.4 GB 解码块常驻（只读 key 的扫描也一样）。改为按表数缩小
    run 目标 + 结果窗口限制在调度器在途预算内 → **峰值 2518 → 276 MiB，时间 49.8 → 39 s**
    （顺带消除了抖动）。
  - JE 侧区间文件句柄跨区块复用（`RegionCache`，每 worker 一份，无锁）：
    11,192 区块读取 **5.77 s → 0.63 s（9×）**。
  - Java 区间文件按区块流式写出（原来整个维度写完后才落盘，峰值 700 MB+）。
  - 调色板/高度图位打包改成「一次填满一个 word」（散列式逐方块读改写 → 分组写入，
    打包循环 **2.5–3.2×**）；解包侧对称改写。
  - 每 worker 复用 NBT 编码缓冲（`encode_into`），不再每区块分配几百 KiB。
  - JE→BE 结束时压缩 LevelDB：同一世界 **26 MB → 2.7 MB**（Chunker 3.5 MB），
    客户端首次打开不再合并几十张小 SST。
  - 负收益并回退：AVX2 位打包（每 word 横向归约使其只有分组标量的 0.07–0.09×）、
    高度图按列自顶向下提前退出（跨步访存，39.5 s → 47.1 s）、跨区块映射缓存
    （区块内调色板槽位已经足够，40.8 s vs 39.5 s）、JE 侧槽位缓存（4.69 → 4.65 s，
    属于噪声，不保留）。

  JE 侧读取路径、扫描策略与打包内核的对比数据在 `core/examples/`：
  `read_bench`、`chunk_read_bench`、`je_read_bench`、`pack_bench`、`batch_bench`。

  **测量口径（重要）**：上表是端到端耗时，两个转换器实际处理的区块数并不相同
  （JE→BE：WorldPort 只转 6,968 个完全生成区块，Chunker 记录 7,312 个；BE→JE：
  108,484 vs 111,744），因此应读作「同输入的实际耗时比」而不是「算法性能倍数」。
  `scripts/bench.ps1` 现在报中位数墙钟 + 多次运行的最大峰值工作集，并标注
  `PeakWorkingSet64` 是 Windows 工作集而非跨平台 RSS。做严格对比需要固定版本、
  输入、目标格式、实际区块数与功能范围，且同机同负载连续运行。
- ✅ M6 插件：
  - `native/` 实现 `bmcbl_native_entry` ABI（1.0）：单实例、64 KiB 命令 / 256 KiB 事件批次、
    事件环 + 序号 + gap 标记、`cancel_command` 直接置取消标志（不受命令队列阻塞）、
    销毁时 join 工作线程并静默回调。
  - `plugin/` 是独立 workspace 的 `.bmcblx`（`worldport`，工具页图标 `network`，order 10）：
    源/输出目录浏览与检测、方向与目标版本、转换内容开关、高级项（并行度/压缩/覆盖）、
    进度、警告与结果摘要、输出目录打开、原生摘要保存；离开页面任务继续，回来用
    `Snapshot` 恢复进度。
  - 打包：`./scripts/build-plugin.ps1` 生成 `dist/worldport-0.1.0.bmcblx` 与
    `dist/native/windows-x86_64-msvc/worldport_native.dll`（DLL 摘要烘进 Wasm）。
- ✅ M9 村庄 / POI / 结构（按能力分别标注，不统称 1:1）：
  - **POI（完整转换）**：Bedrock `VILLAGE_*_POI` 按村民存、Java `poi/r.X.Z.mca` 按坐标存，
    转换是「展平 + 按坐标去重」而不是复制。`free_tickets = Capacity - OwnerCount`（钳到 0，
    capacity 为 0 视为单人）；Java 的票据是「剩余可用」，与 Bedrock 的归属计数相反。
    真实 1.20.12 存档：1097 条 / 480 区块 / **0 重复坐标**，604 床与源数据完全一致；
    443 个集合点 listing 归并为 40 个钟（同一村庄每个村民都会列出它）。
    Bedrock 独有（蜂巢/磁石/避雷针/下界传送门）无 Java POI 类型，计数后丢弃，不写未知类型。
  - **村庄运行状态（语义重建）**：Bedrock 的村庄（边界/居民）无法从方块推导，也不伪造
    `VILLAGE_*` 记录；JE→BE 改为统计地形中的兴趣点并说明 Bedrock 会在区块加载时自行重建村庄。
  - **村民（完整转换）**：15 个职业、7 个类型、等级/层级（Bedrock 0 基、Java 1 基）双向映射；
    未知 `SkinID` 保持失业并报告，不猜职业。真实存档 784 个村民 0 个缺失 `VillagerData`。
  - **结构生成区（部分保留）**：`HardcodedSpawners`（tag 0x39）是二进制生成盒，不是 NBT，
    没有 piece 列表 / 战利品 / `/locate` 键。Java 有 `Children` 边界时写生成区
    （fixture：`[-8,40,-8]..[8,60,8]` 精确往返）；Bedrock→Java **不伪造** `StructureStart`
    ——伪造的 piece 会让 `/locate` 指向不存在的结构并干扰后续生成，改为统计并说明
    （真实存档 2422 个区，与独立 dump 的 276/49/5 一致）。
  - 结构模板（`.nbt` ↔ `.mcstructure`）未做，v1 边界不变。
  - 结构统计走**计数器**而非警告行：同一世界有 2049 条方块级警告，超过报告 2048 行上限。

## 数据与许可

- [Chunker](https://github.com/HiveGamesOSS/Chunker)（MIT)：转换流程参考、生物群系 ID 表、
  方块朝向/状态取值表（`BedrockStateTypes`/`BedrockStateGroups`，与官方 wiki 语义一致）。
- [Minecraft Wiki](https://minecraft.wiki)：Bedrock/Java 方块状态与 `weirdo_direction` 等语义校验。
- 地图颜色与计划刻格式同样以真实存档 + Chunker 解析器为准；计划刻采用统一模型而非
  直接搬运 NBT（两版调度规则不同，只能保留未执行事件及其时间关系）。

## 性能记录

- 测量脚本：`./scripts/bench.ps1 -World <dir> -Out <dir> -Runs 3`，加 `-Chunker`
  用同样输入跑官方 CLI。脚本固定同时输出**墙钟时间、峰值工作集、产物大小**——
  只看速度会把「用内存换时间」的改动当成优化。
- 内存定位：`World::open` 后 4.2 MiB，列 111,744 个区块坐标 32.4 MiB（并行扫描的
  run 缓冲已按表数收敛），整个 108k 区块转换峰值 276 MiB。Chunker 同任务 3943 MiB
  （JVM 堆上限 8 GB）。
- 映射表编译期烘焙：8.2 MB JSON → 99 KB 二进制 → 15.8 KB LZ4，运行时一次解压。
- 地图调色板匹配实测（真实存档 64 张地图、1,048,576 像素、79 种唯一颜色）：
  逐像素线性搜索 479.9 ms，按颜色 memo 后 **1.81 ms（264×）**。
  因此没有引入 SIMD 内核：memo 已经把这段降到可忽略，fearless_simd 再快也没有
  空间（避免负优化）。NBT 解析本身已通过 `bedrock-nbt` 的 `simd` feature 使用
  fearless_simd。
- Mojang `bedrock-samples`：Bedrock 方块状态清单（经 `bedrock-world` 内置 schema）。
- `misode/mcmeta`：Java 版方块注册表（`mappings/data/java/<version>/blocks.json`）。
- BMCBL `crates/`：Bedrock 存储实现（`vendor/`，见 `vendor/SYNC.md`）。

本项目自身以 GPL-3.0 发布。
