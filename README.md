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
| 结构与村庄 | 不支持 | v1 同样不支持（报告中明示） |
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
- ✅ M5 性能：
  - 区块转换按 `--jobs` 并行（rayon 线程池，`0` = 全部逻辑核心），结果按输入顺序
    提交，UID 用原子计数保持唯一。
  - Bedrock 侧改成**批量点读**：一次 `get_many` 取齐该区块固定 tag 与 24 个 subchunk，
    取代逐表前缀扫描；区块坐标列表只扫描一次（原来每个维度扫一次）。
  - 区间文件按区块并行压缩（zlib）；每节方块只对**唯一的调色板条目**做一次映射，
    不再逐方块构造属性表。
  - 与官方 Chunker 1.21.0 CLI 同输入实测（Windows 11，16 逻辑核 / 8 物理核，
    `[profile.native-release]`）：

    | 方向 | 输入 | WorldPort | Chunker 1.21.0 |
    | --- | --- | --- | --- |
    | JE→BE | 调试 JE 世界 11,192 个区块（6,968 个完全生成） | **3.78 s** | 6.39 s（7,312 区块，含未完全生成） |
    | BE→JE | 真实 1.20.12 存档 443 MB（108,484 个区块，29.8 亿方块） | **51.3 s** | 82.5 s（111,744 区块） |

    同一 BE→JE 输入上 `--jobs 1` 为 2.43 ms/区块、并行后 0.52 ms/区块（4.7×，受
    8 个物理核限制）。BE→JE 产物 709 MB，与 Chunker 的 746 MB 同量级；实体、方块实体、
    地图、玩家、计划刻都是额外转换的（Chunker 不转实体/地图/tick）。
  - 回环保真度（JE→BE→JE 与原世界对比，我们自己的转换器读回两个产物）：
    WorldPort 3.0% 方块差异、0 实体/tick/群系差异；Chunker 2.8% 方块差异、504 个方块
    实体差异、68 个未映射方块。两者的剩余方块差异都集中在墙/楼梯连接状态归一化。
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

## 数据与许可

- [Chunker](https://github.com/HiveGamesOSS/Chunker)（MIT)：转换流程参考、生物群系 ID 表、
  方块朝向/状态取值表（`BedrockStateTypes`/`BedrockStateGroups`，与官方 wiki 语义一致）。
- [Minecraft Wiki](https://minecraft.wiki)：Bedrock/Java 方块状态与 `weirdo_direction` 等语义校验。
- 地图颜色与计划刻格式同样以真实存档 + Chunker 解析器为准；计划刻采用统一模型而非
  直接搬运 NBT（两版调度规则不同，只能保留未执行事件及其时间关系）。

## 性能记录

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
