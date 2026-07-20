# CCopy 多端同步设计（WebDAV）

> 日期：2026-07-20
> 状态：设计已确认，待实现

## 一、目标

解决多台电脑间剪贴板历史断联问题。用户在公司、家里、笔记本上复制的文本/图片能在各端互通，无需手机中转。

## 二、方案选型

**WebDAV 后端 + 全量快照事件流**

- 后端：对接坚果云等现成 WebDAV 服务，零运维
- 同步模型：每端独立文件 + 客户端合并，避免多端写冲突
- 事件格式：全量快照，每条自包含，不依赖历史回放

### 为什么不用 UUID

现有架构用 `hash`（内容哈希）做唯一键，已有 `UNIQUE INDEX (kind, hash)`，`upsert_item` 按 `(kind, hash)` 去重。hash 全局唯一且跨设备一致，直接当同步 ID 即可，不引入 UUID。

### 为什么用全量快照而非增量事件

增量事件（只带变化字段）要求严格按序回放，丢失或乱序即出错，且「取最新一条」会丢字段（如先 create 带正文、再 update 只带备注，只执行最后一条会失去正文）。全量快照每条自包含，合并逻辑极简：按 id 分组取 `updated_at` 最大的那条直接落库。剪贴板记录字段不多，体积代价可接受。

### 为什么用物理删除而非软删除

现有 `delete_item` 是物理删除，贴合现有架构。删除事件处理即丢，不保留软删除缓存。「删除后又复制相同内容导致复活」符合用户预期（重新复制了同样东西出现在历史里是合理的），不值得为此引入软删除缓存。

## 三、数据结构

### 同步事件（全量快照）

```rust
enum SyncEvent {
    Upsert {
        id: String,                  // = hash，全局唯一
        source_device: String,       // 来源设备名，防回环
        kind: String,                // text/html/rtf/image/files
        preview: String,
        text_content: Option<String>,
        plain_text: Option<String>,
        blob_path: Option<String>,   // 图片指向 images/<hash>.png
        note: Option<String>,
        created_at: i64,
        updated_at: i64,             // 合并依据，LWW
    },
    Delete {
        id: String,                  // = hash
        source_device: String,
        updated_at: i64,
    },
}
```

upsert 事件始终是完整快照；delete 事件只需 id + updated_at。

### JSONL 示例

```jsonl
{"op":"upsert","id":"abc123","source_device":"office-pc","kind":"text","preview":"hello","text_content":"hello world","plain_text":"hello world","note":null,"created_at":1700000000000,"updated_at":1700000000000}
{"op":"upsert","id":"abc123","source_device":"office-pc","kind":"text","preview":"hello","text_content":"hello world","plain_text":"hello world","note":"重要","created_at":1700000000000,"updated_at":1700000005000}
{"op":"delete","id":"abc123","source_device":"home-pc","updated_at":1700000010000}
```

## 四、WebDAV 文件结构

```
CCopy/sync/
  ├── devices/
  │   └── <device_name>/
  │       ├── 2026-07.jsonl      # 当月文件，持续追加
  │       ├── 2026-06.jsonl      # 归档
  │       └── ...
  ├── images/
  │   └── <hash>.png             # 图片，哈希命名天然去重
  └── manifest.json              # 各端最后同步位置（可选）
```

### 文件管理

- **按月轮转**：单文件上限约 3-6MB（月复制量 × 单条大小）
- **etag 增量**：PROPFIND 拿文件 etag/last-modified，本地缓存，老月份文件 etag 不变不下载
- **按月自动清理**：超过保留期（默认 3 个月）的归档文件删除
- **图片默认关闭**：流量考虑，设置页可开关

## 五、同步语义

### 三种操作

#### 1. 新增 / 修改
本机复制新内容或改备注 → 本地 `upsert_item` → 读出完整 item → 生成全量 upsert 事件 → 追加到 `devices/<本机名>/<YYYY-MM>.jsonl`

其他端拉取 → 用 `upsert_item` 直接落库（已存在就 UPDATE，不存在就 INSERT），复用现有去重逻辑，零改造。

#### 2. 删除
本机删除 → 本地 `delete_item`（物理删）→ 追加 delete 事件

其他端拉到 delete 事件 → 本地 `delete_item`（物理删）。删除事件处理完即丢弃，不保留缓存。

#### 3. 拉取合并
1. 读所有设备所有月份的 jsonl
2. 按 `id`（=hash）分组所有事件
3. 每组取 `updated_at` 最大的全量快照
4. 该快照是 upsert → `upsert_item` 落库；是 delete → `delete_item` 物理删

### 防回环
每条事件带 `source_device`，收到来自本机的事件跳过，不重复处理自己刚写的。

### 冲突解决：Last-Write-Wins
多端同时改同一 hash 的备注，按 `updated_at` 最大的胜。时间戳用各端本地时间（NTP 同步，误差可接受）。极端同毫秒情况用 `content_hash` 做 tiebreaker，保证所有端结果一致。

## 六、设置页 Tab 化改造

### 现状
设置页是单页 ScrollView，堆了 6 块（快捷键、自启、清理规则、统计、手动清理、更新+关于）。加同步后更挤。

### Tab 划分

| Tab | 包含内容 | 来源 |
|---|---|---|
| 通用 | 唤起快捷键、开机自启 | 现有 |
| 清理 | 自动清理规则（最大记录数、保留天数）、手动清理（清空未标记、清空全部） | 现有合并 |
| 同步 | WebDAV 配置、同步开关、内容选择、状态、保留期 | 新增 |
| 关于 | 统计、更新检查、版本信息 | 现有 |

### UI 结构
```
设置窗口
├── 标题栏（保留：标题 + 关闭按钮）
├── Tab 栏（新增：通用/清理/同步/关于，分段控件）
├── 内容区（根据 active_tab 切换显示）
└── 二次确认弹层（保留）
```

- `in-out property <int> active_tab` 控制切换（0/1/2/3）
- 内容区用 `if` 条件渲染四个 VerticalLayout
- 保留现有拖拽、快捷键录制、二次确认弹层机制

### 同步 Tab 字段
- **启用同步**：开关
- **WebDAV 地址**：文本输入（如 `https://dav.jianguoyun.com/dav/CCopy`）
- **用户名**：文本输入
- **密码**：密码输入（坚果云用应用专有密码）
- **设备名**：文本输入（默认主机名）
- **同步内容**：文本（固定开）/ 图片（开关，默认关）
- **保留期**：数字输入（月，默认 3）
- **状态**：已连接 / 同步中 / 断开 / 最后同步时间
- **立即同步**：按钮（手动触发一次拉取+推送）

## 七、模块结构

```
src/
├── sync/               # 新增同步模块
│   ├── mod.rs          # 模块入口 + 同步协调器（推送/拉取/定时器）
│   ├── webdav.rs       # WebDAV 客户端封装（PROPFIND/GET/PUT/DELETE）
│   ├── event.rs        # SyncEvent 定义 + JSONL 序列化
│   ├── merge.rs        # 拉取合并逻辑（分组取最新全量快照）
│   └── config.rs       # 同步配置（地址/账号/设备名等）
├── settings.rs         # 改造：加载/保存同步配置
├── ui/settings.slint   # 改造：tab 化 + 同步 tab
├── main.rs             # 改造：接入同步协调器
└── clipboard_history.rs # 改造：watcher 后触发推送
```

## 八、新增依赖

按项目规范用 `cargo add` 安装最新稳定版：
- `reqwest`（HTTP/WebDAV 请求，features: `blocking` 或异步）
- `serde` + `serde_json`（如 Cargo.toml 还没有）
- `sha2`（确认 [common.rs](file:///d:/200-my/CCopy/src/common.rs) 现有 hash 逻辑是否可复用）

## 九、同步策略细节

### 推送时机
- 本机复制新内容（watcher 触发）→ 异步推送
- 改备注（`update_note`）→ 异步推送
- 删除（`delete_item`）→ 异步推送

### 拉取时机
- 定时器每 5 秒拉取一次（PROPFIND 检测变化的设备文件）
- 设置页「立即同步」按钮手动触发
- 应用启动时拉取一次

### 断网处理
- 推送失败：本地标记 `synced=false`，下次重试
- 拉取失败：跳过本次，下次定时器重试
- 不阻塞主流程，同步失败不影响本地剪贴板功能

### 图片同步（可选）
- 默认关闭
- 开启后：本机复制图片 → 上传到 `images/<hash>.png` → upsert 事件带 `blob_path`
- 拉取时按需下载缩略图，原图按需拉取

## 十、实现步骤

### 阶段一：设置页 Tab 化（独立可验证）
1. 改 `settings.slint` 加 tab 栏 + 四个内容区
2. 迁移现有控件到对应 tab（清理 tab 合并自动+手动）
3. Rust 侧加 `active_tab` 状态
4. 验证：现有功能在 tab 化后正常工作

### 阶段二：同步配置层
1. `sync/config.rs` 定义配置结构
2. `settings.rs` 加载/保存同步配置
3. 同步 tab UI 接入配置读写
4. 验证：配置能持久化保存

### 阶段三：WebDAV 客户端
1. `sync/webdav.rs` 封装 PROPFIND/GET/PUT/DELETE
2. 联调坚果云
3. 验证：能上传下载文件

### 阶段四：同步事件层
1. `sync/event.rs` 定义 SyncEvent + JSONL 读写
2. `sync/merge.rs` 合并逻辑
3. 验证：合并逻辑单元正确

### 阶段五：同步协调器
1. `sync/mod.rs` 推送/拉取/定时器
2. 接入 `clipboard_history` watcher、`delete_item`、`update_note`
3. 验证：多端文本同步工作

### 阶段六：图片同步（可选）
1. 上传/下载 `images/<hash>.png`
2. 缩略图处理
3. 验证：图片跨端显示

## 十一、验证清单

- [ ] 单端：复制文本 → 写本地 + 追加 jsonl
- [ ] 单端：改备注 → 追加 upsert 事件（全量快照）
- [ ] 单端：删除 → 物理删本地 + 追加 delete 事件
- [ ] 多端：A 端复制 → B 端拉取后出现
- [ ] 多端：A 端改备注 → B 端拉取后备注更新
- [ ] 多端：A 端删除 → B 端拉取后本地也删
- [ ] 多端：同时改备注 → LWW 取最新
- [ ] 防回环：自己的事件不重复处理
- [ ] 断网：推送失败不阻塞本地功能
- [ ] 文件轮转：跨月后新文件生成
- [ ] 自动清理：超期归档删除
- [ ] 设置页 tab 切换正常
- [ ] 图片同步（如开启）

## 十二、关键取舍总结

| 项 | 决策 | 理由 |
|---|---|---|
| 后端 | WebDAV（坚果云/自建） | 零运维，免费可用，通用协议 |
| 同步 ID | hash（复用现有字段） | 全局唯一，跨设备一致，零改造 |
| 事件格式 | 全量快照 | 自包含，合并简单，容忍乱序丢失 |
| 同步模型 | 每端独立文件 + 客户端合并 | 避免多端写冲突 |
| 删除 | 物理删除 + delete 事件 | 贴合现有架构，无软删除缓存堆积 |
| 冲突解决 | Last-Write-Wins | 自用场景，简单可靠 |
| 文件轮转 | 按月分文件 | 单文件可控，老文件 etag 不变零成本 |
| 清理 | 按月自动清理（默认 3 月） | 避免无限增长 |
| 图片 | 默认关闭 | 流量可控 |
| 设置页 | Tab 化（通用/清理/同步/关于） | 内容增多后清晰分区 |
