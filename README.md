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
- ⏳ M3：实体/物品/玩家/地图/tick。
- ⏳ M4：版本升降级与剔除。
- ⏳ M5：性能对比（同一输入跑 Chunker CLI）。
- ⏳ M6：`.bmcblx` 插件 + 原生 DLL，注册到 BMCBL「工具」页。

## 数据与许可

- [Chunker](https://github.com/HiveGamesOSS/Chunker)（MIT)：转换流程参考、生物群系 ID 表、
  方块朝向/状态取值表（`BedrockStateTypes`/`BedrockStateGroups`，与官方 wiki 语义一致）。
- [Minecraft Wiki](https://minecraft.wiki)：Bedrock/Java 方块状态与 `weirdo_direction` 等语义校验。
- Mojang `bedrock-samples`：Bedrock 方块状态清单（经 `bedrock-world` 内置 schema）。
- `misode/mcmeta`：Java 版方块注册表（`mappings/data/java/<version>/blocks.json`）。
- BMCBL `crates/`：Bedrock 存储实现（`vendor/`，见 `vendor/SYNC.md`）。

本项目自身以 GPL-3.0 发布。
