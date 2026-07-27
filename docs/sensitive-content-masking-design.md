# 敏感内容脱敏扩展方案

## 1. 文档信息

- 状态：方案设计
- 目标版本：待排期
- 适用范围：剪贴板主列表、快速粘贴窗口、类型筛选、内容编辑
- 新增语义类型：`secret`（界面显示为“密钥”）

## 2. 背景

Clippi 目前只对邮箱和电话预览做脱敏：

- 邮箱：显示本地部分前 3 个字符、固定 `****` 和完整域名。
- 电话：显示前 3 个字符、固定 `****` 和后 4 个字符。
- 数据仍以原文保存，复制、粘贴和编辑使用原文。
- 主列表卡片和快速粘贴窗口都调用
  `core::types::mask_sensitive_preview`，但主列表卡片还会再次查找并拆分
  `****`，以便给掩码应用独立样式。
- “信息”筛选的内部键为 `contact`，当前只匹配
  `meta_type IN ('email', 'phone')`。

需要把密码、API Key、访问令牌、私钥等疑似密钥纳入敏感内容识别，在“信息”
类型下新增“密钥”语义类型，并统一脱敏算法和预览渲染，避免不同界面出现规则漂移。

## 3. 目标与非目标

### 3.1 目标

1. 新复制的疑似密码、API Key、访问令牌和私钥能自动识别为“密钥”。
2. 密钥值默认只显示前 3 个和后 4 个字符，中间固定显示 `****`。
3. “信息”筛选同时包含邮箱、电话和密钥。
4. 编辑面板允许用户手动把纯文本改为“密钥”，也允许纠正误识别。
5. 主列表与快速粘贴复用同一套脱敏数据结构和 GPUI 组件。
6. 不改变实际复制、粘贴、收藏、同步和编辑所使用的原始内容。
7. 识别与脱敏不记录敏感原文日志。

### 3.2 非目标

- 本方案不是数据库加密、端到端加密或系统级密码保险箱。
- 本方案不阻止用户通过复制、粘贴或编辑页主动查看原文。
- 本方案不承诺识别所有无上下文的自然语言密码。
- 首期不新增密码强度检测、泄露密码查询或密钥有效性联网验证。

脱敏的安全边界是减少列表预览和快速粘贴界面的旁观泄露。SQLite、同步载荷和
系统剪贴板仍可能包含原文；若要防止本地文件读取或同步端泄露，需要单独设计加密方案。

## 4. 核心设计决策

### 4.1 数据模型

沿用现有的“主类型 + 语义子类型”模型：

```text
content_type = "plain_text"
meta_type    = "secret"
```

新增 `DisplayKind::Secret`，所有 UI 通过 `ClipboardItem::display_kind()` 判断，
不在各组件内直接散落 `meta_type == "secret"` 分支。

不新增数据库列，现有 `meta_type` 字符串列和同步模型可以承载 `secret`，因此不需要
结构迁移。需要同步更新源码注释、白名单和测试中列出的合法 `meta_type` 值。
其中 `ClipboardItem.meta_type` 的注释应同时补齐已在使用但尚未列出的 `transfer`，
避免文档继续落后于实际协议。

自动识别结果只保存统一的 `secret` 类型。`Password`、`ApiKey`、`AccessToken`、
`PrivateKey` 等细分类仅用于检测策略、测试和调试统计，不持久化，避免扩大数据协议。

### 4.2 “信息”筛选

当前产品没有嵌套类型筛选结构，“信息”本身就是聚合筛选项。因此本次不增加新的顶层
筛选按钮，而是把 `contact` 的语义由“邮箱/电话”扩展为“邮箱/电话/密钥”：

```sql
meta_type IN ('email', 'phone', 'secret')
```

同时，纯文本筛选必须排除 `secret`：

```sql
content_type = 'plain_text'
AND meta_type NOT IN ('email', 'phone', 'secret', 'link', 'path', 'color')
```

这样可以满足“在信息类型下新增密钥类型”，并保持用户已有的类型筛选顺序和可见性配置
不变。编辑面板和卡片类型标签则单独显示“密钥”。

`contact` 只是已持久化到 `type_filter_config` 的内部兼容键，界面文案已经是更宽泛的
“信息 / Info”。本期明确保留该内部键，不迁移为 `sensitive`，否则需要处理用户已有的
顺序、显隐设置和多设备配置合并。源码注释需要说明它是“信息聚合筛选”而不是严格意义的
“联系方式”。若以后全面整理筛选协议，再通过带兼容映射的独立迁移重命名。

纯文本条件本期仍保留 `NOT IN` 兼容策略，而不直接改成 `meta_type = ''`。原因是旧版本
遇到未来新增或未知 `meta_type` 时仍可把它安全回退为普通文本；白名单式空值判断会让这些
条目从所有类型筛选中消失。为减少排除列表继续散落，实施时应把语义子类型集合集中为常量
或统一的 SQL 构造方法，并补充“新增子类型必须同步更新纯文本排除规则”的注释和测试。

### 4.3 显示与实际操作分离

- 列表卡片、快速粘贴预览：显示脱敏内容。
- 普通复制、快速粘贴、快捷键粘贴：使用 `ClipboardItem.full_text` 原文。
- 编辑页：进入编辑后显示原文，以便用户修改；类型下拉增加“密钥”。
- 搜索：首期保持现有行为，仍对原始可搜索文本匹配，但任何命中都不能把被隐藏字符
  渲染出来。
- 备注：仍按现有优先级展示，不能把原始密钥自动写入备注。

## 5. 密钥识别设计

### 5.1 检测结果

建议在 `core::types` 中增加与 UI 无关的检测模型：

```rust
pub enum SecretKind {
    Password,
    ApiKey,
    AccessToken,
    PrivateKey,
    Credential,
}

pub enum SecretConfidence {
    High,
    Medium,
}

pub struct SecretMatch {
    pub kind: SecretKind,
    pub confidence: SecretConfidence,
    pub value_ranges: Vec<std::ops::Range<usize>>,
}

pub const SECRET_DETECTION_RULE_VERSION: u16 = 1;

pub fn detect_secret(text: &str) -> Option<SecretMatch>;
```

`value_ranges` 指向真正需要隐藏的值，而不是一律隐藏整段文本。例如
`API_KEY=sk-proj-xxxx` 应保留 `API_KEY=`，只脱敏等号后的值。范围只在内存中使用，
不写入数据库；展示时对原文重新执行同一个确定性检测器。

范围使用 UTF-8 字节边界存储，但必须由 `char_indices()` 生成并验证边界，禁止直接按
任意字节截断。

`detect_secret` 必须是无 IO、无时间状态、无随机数、无全局可变配置的纯函数。同一版本
内，相同输入必须得到完全相同的类型、置信度和范围顺序；范围在返回前按起点排序、去重并
合并重叠区间。

规则变化时递增 `SECRET_DETECTION_RULE_VERSION`，用于：

- 使按内容哈希缓存的检测结果失效。
- 标记历史回填所使用的规则版本。
- 驱动固定测试向量的兼容回归。

规则版本不会持久化到每条剪贴板记录，因此它不能保证跨版本的范围永远相同。安全约束是：
已标记为 `secret` 的条目在新规则无法重建范围时，必须退化为整段脱敏，绝不能回退显示
原文；已有高置信度规则不得在无迁移和回归测试的情况下删除或放宽。若未来需要逐条冻结
历史范围，再单独设计持久化的检测元数据，首期不扩大数据协议。

### 5.2 识别优先级

剪贴板文本检测顺序调整为：

```text
Files
→ Image
→ Link
  └─ URL userinfo / 敏感查询参数：保留 link 类型，只局部脱敏
→ Secret
→ Path
→ Color
→ Email / Phone
→ RichText
→ Markdown
→ PlainText
```

这里明确采用“链接语义优先”：

- 只要整段文本是合法 URL，就继续保存为 `meta_type = "link"`，保留域名、favicon、
  页面标题、打开链接等能力。
- URL 中的 `scheme://user:password@host`、`token=`、`api_key=` 等敏感部分只影响
  预览脱敏，不把整条记录改成 `secret`，因此它归入“链接”筛选而不是“信息”筛选。
- 域名提取和 favicon 请求必须使用清洗后的 host，禁止把 userinfo、密码或敏感查询参数
  传入 favicon 服务。
- URL 路径和查询参数预览通过结构化敏感片段渲染，不能继续直接展示原始 query。

非 URL 文本再执行 Secret 检测，并放在 Path、Color、Email/Phone 之前，以正确处理
Bearer Token、`.env` 配置和特殊前缀令牌。密钥规则仍需足够保守，避免普通路径和颜色被
抢先分类。

### 5.3 高置信度规则

满足任一高置信度规则即可自动标记为 `secret`：

1. **明确字段名**

   单行、多行 `.env`、JSON、YAML 或常见配置片段中，字段名命中：

   ```text
   password, passwd, pwd, passphrase
   secret, client_secret
   api_key, apikey
   access_key, secret_key
   access_token, auth_token, refresh_token
   private_key
   ```

   支持大小写、`-`/`_`/驼峰差异，以及 `=`、`:` 分隔。字段值去除成对引号后参与
   判断和脱敏；字段名、分隔符及引号可以保留。

2. **明确认证头**

   ```text
   Authorization: Bearer <token>
   Bearer <token>
   Authorization: Basic <credentials>
   ```

3. **已知密钥格式**

   首期维护一组集中式规则表，至少覆盖：

   - GitHub Token：`ghp_`、`gho_`、`ghu_`、`ghs_`、`ghr_`、
     `github_pat_`。
   - AWS Access Key ID：`AKIA`、`ASIA` 等合法前缀和长度组合。
   - OpenAI Key：`sk-` 及带项目/服务账号前缀的形式。
   - Stripe 私密或受限 Key：`sk_`、`rk_`；不把明确的公开 `pk_` Key
     自动视为密钥。
   - Slack Token：`xoxb-`、`xoxp-`、`xoxa-`、`xoxr-`、`xoxs-`。
   - Google API Key：`AIza` 加合法长度和字符集。
   - JWT/JWS：三个 Base64URL 段，且头部能解码为合理 JSON。

   规则必须同时校验长度、字符集和边界，不能只用前缀判断。

4. **私钥块**

   识别 `-----BEGIN ... PRIVATE KEY-----` 到对应 `END` 的完整块。标题和尾部标记可
   保留，正文按密钥值脱敏；禁止在日志中打印正文。

5. **URL 内嵌凭据**

   对形如 `scheme://user:password@host` 的文本仅脱敏密码部分；用户名是否显示沿用
   URL 预览策略，但不得传给 favicon 服务。对 query 中字段名命中
   `token`、`api_key`、`access_token`、`password` 等规则的值也做局部脱敏。

   这条规则只生成敏感显示范围，不改变条目的 `meta_type = "link"`。URL 必须使用
   结构化解析器提取 scheme、userinfo、host、path 和 query；禁止用简单字符串切分处理
   百分号编码、IPv6、端口或 `@` 转义等边界。若引入 `url` crate，应作为直接依赖锁定
   版本并补充解析失败的安全降级测试。

### 5.4 疑似独立密码规则

无字段名、无厂商前缀的独立字符串误报风险最高。建议只有同时满足以下条件时才自动识别：

- 去除首尾空白后为单行、内部无空白。
- 字符数为 12～256。
- 至少包含大写字母、小写字母、数字、符号中的 3 类。
- 不符合 URL、邮箱、电话、颜色、UUID、常见哈希、文件路径和自然语言模式。
- 不能由单一字符或短片段大量重复组成。
- 通过保守的随机性/熵阈值。

这类结果内部标记为 `Medium`。首期可以选择：

- 推荐：仍自动标记为 `secret`，但用更严格阈值，并通过负例测试控制误报。
- 若实测误报偏高：只让高置信度规则自动标记，用户可在编辑面板手动选择“密钥”。

不要把“任意包含数字和符号的 8 位字符串”直接当作密码，否则订单号、版本号、短代码和
普通随机 ID 会产生大量误报。明确字段名下的短密码不受 12 字符限制。

### 5.5 多值文本

配置片段可能同时包含多个敏感值：

```text
API_KEY=abcdefghijk
PASSWORD="correct-horse-battery-staple"
REGION=ap-east-1
```

检测器应返回全部敏感值范围。预览保留非敏感字段和换行，对每个敏感值分别脱敏；不能只
处理第一个值，也不能因为整条记录是 `secret` 就把 `REGION` 等非敏感内容全部替换。

私钥块首期直接使用整段摘要预览，例如 `PRIVATE KEY · ****`，不显示正文的前 3、后 4
字符。PEM 正文过长且尾部通常不具备辨识价值，部分展示收益低于误泄露风险；私钥类型标签
从 `BEGIN ... PRIVATE KEY` 标题派生，不读取或展示正文。此规则是“前 3 + 后 4”的明确
安全例外。

### 5.6 误识别纠正

- 编辑面板类型选项增加“密钥”。
- 用户可把误识别的密钥改回“文本”或其他类型。
- 用户手动选择的非空 `meta_type` 优先于自动检测，重新载入和数据回填不得覆盖。
- 未来如增加“重新识别”功能，应由用户显式触发，不在每次启动时反复改写手动分类。

## 6. 通用脱敏方法

### 6.1 通用核心 API

不建议继续让 UI 从格式化字符串中查找 `****`。建议把脱敏结果结构化：

```rust
pub const DEFAULT_MASK: &str = "****";

pub struct MaskedValue {
    pub prefix: String,
    pub mask: &'static str,
    pub suffix: String,
}

pub enum SensitivePreviewPart {
    Plain(String),
    Masked(MaskedValue),
}

pub struct MaskRule {
    pub visible_prefix_chars: usize,
    pub visible_suffix_chars: usize,
}

pub fn mask_middle(value: &str, rule: MaskRule) -> MaskedValue;

pub fn sensitive_preview_parts(
    text: &str,
    meta_type: &str,
) -> Vec<SensitivePreviewPart>;
```

职责划分：

- `mask_middle`：只负责字符安全的通用“保留头尾、隐藏中间”算法。
- `sensitive_preview_parts`：根据邮箱、电话或密钥规则定位敏感值，输出可渲染分段。
- 密钥检测器：只判断类型和敏感范围，不依赖 GPUI。
- UI 组件：只渲染 `SensitivePreviewPart`，不再识别内容或解析星号。

`sensitive_preview_parts` 采用“内部重新检测”的实现，不给 `ClipboardItem` 增加范围
缓存字段：

- `secret`：重新调用纯函数 `detect_secret()`；检测失败时整段输出为一个
  `Masked`，禁止返回 `Plain` 原文。
- `link`：调用 URL 敏感范围提取器，只对 userinfo 密码和敏感 query 值输出
  `Masked`，其余 URL 片段保持链接展示。
- `email`、`phone`：使用各自已有的结构规则定位范围。

为避免同一帧重复解析，可以使用
`(content_hash, meta_type, SECRET_DETECTION_RULE_VERSION)` 作为短生命周期缓存键；缓存
属于 UI/预览服务层，不进入持久化模型。

### 6.2 脱敏规则

| 类型 | 显示规则 | 示例 |
| --- | --- | --- |
| 电话 | 前 3 + `****` + 后 4 | `138****5678` |
| 邮箱 | 本地部分前 3 + `****` + 完整域名 | `abc****@example.com` |
| 密钥/密码 | 值前 3 + `****` + 值后 4 | `sk-****WXYZ` |

密钥和电话默认使用 `MaskRule { 3, 4 }`。掩码固定为四个星号，不反映原文长度。

短值必须采用安全降级，不能像当前电话逻辑一样因为长度不足而显示原文：

- 字符数大于 7：显示前 3、`****`、后 4。
- 字符数等于或小于 7：仅显示 `****`。
- 空值：不产生密钥匹配。

所有长度都按 Unicode 字符而不是 UTF-8 字节计算。即使密码中包含中文或 emoji，也不能
发生切片 panic 或半字符泄露。

这会有意改变手动标记、导入或旧数据中不大于 7 个字符的 `phone` 显示行为：从原文改为
`****`。虽然自动电话识别当前不会产生这么短的号码，这仍属于用户可见的安全修正，必须
写入 CHANGELOG/发布说明，并回归验证复制和粘贴仍使用完整原文。

### 6.3 邮箱兼容

邮箱继续保留完整域名，保持现有产品行为。若本地部分不超过 3 个字符，建议改为只显示
第 1 个字符和 `****@domain`，避免现有实现完整暴露短本地部分。该调整属于安全修正，
实施前可通过产品验收确认是否需要完全保持旧样式。

## 7. 通用 GPUI 组件

新增 `src/ui/components/sensitive_text.rs`，提供统一的敏感预览渲染组件，例如：

```rust
SensitiveText::new(parts)
    .search_terms(search_terms)
    .text_color(theme.text_1)
    .mask_color(theme.text_3)
    .font_size(px(13.))
    .font_weight(FontWeight::BOLD)
```

组件负责：

- 普通片段、可见前缀和可见后缀的排版。
- 掩码使用弱化颜色统一展示。
- 对可见片段应用现有搜索高亮。
- 隐藏片段永远不传给高亮组件，避免搜索结果渲染出原始字符。
- 支持多个敏感片段、换行和溢出裁切。
- 不持有 `ClipboardItem`、数据库或业务状态，保持 `RenderOnce` 组件轻量。

调用方：

1. `clipboard_card.rs`：邮箱、电话、密钥统一使用 `SensitiveText`，删除手工
   `find("****")` 和 prefix/suffix 拼装。
2. `quick_paste.rs`：`preview_parts` 不再返回已拼好的脱敏字符串；敏感类型走
   `SensitiveText`。普通 URL、路径、图片和文件预览保持原逻辑。
3. URL 预览：域名仍按链接样式展示，路径/query 字幕可使用 `SensitiveText`；favicon
   只接收结构化解析得到的 host。

如果快速粘贴当前结构难以直接复用完整组件，至少必须复用
`sensitive_preview_parts`，并在后续重构为同一组件；不应复制检测或掩码算法。

## 8. 代码影响范围

| 文件 | 主要改动 |
| --- | --- |
| `src/core/types.rs` | 新增 `DisplayKind::Secret` 和映射；更新 `meta_type` 注释，补齐 `secret`、`transfer` |
| `src/core/secret.rs` | 新增检测模型、规则版本、`detect_secret`、URL 敏感范围、结构化脱敏和字符安全算法 |
| `src/core/mod.rs` | 导出 `secret` 模块 |
| `src/platform/clipboard.rs` | 保持 URL 优先；非 URL 文本加入密钥识别并写入 `meta_type = "secret"` |
| `src/core/filters.rs` | “信息”包含 `secret`，纯文本排除 `secret`，集中子类型条件并补充 SQL 测试 |
| `src/core/i18n_keys.rs` | 新增“密钥 / Secret”的编辑类型和卡片类型文案 |
| `src/ui/edit_panel.rs` | 类型选项、类型映射、纯文本编辑类型集合加入 `secret` |
| `src/state/app.rs` | 编辑保存映射加入 `("plain_text", "secret")` |
| `src/ui/components/sensitive_text.rs` | 新增统一敏感内容预览组件 |
| `src/ui/components/mod.rs` | 导出通用组件 |
| `src/ui/clipboard_card.rs` | 接入 `DisplayKind::Secret` 和统一组件，删除手工掩码拆分 |
| `src/ui/quick_paste.rs` | 接入密钥图标、类型和统一敏感预览 |
| `src/services/favicon.rs` / URL helper | favicon 只使用清洗后的 host；URL userinfo 和敏感 query 进入结构化脱敏 |
| `src/core/sync.rs` | 更新 `meta_type` 协议注释和兼容性测试；确认 `secret` 原样透传 |
| `src/core/migration.rs` / DB 层 | 可选的一次性历史数据回填 |
| `Cargo.toml` / `Cargo.lock` | 若采用 `regex`、`url`，声明直接依赖；静态规则使用标准库 `LazyLock` 预编译 |
| `CHANGELOG.md` | 说明新增密钥脱敏，以及短电话从显示原文改为 `****` 的安全修正 |

密钥图标优先从现有内嵌 icon font 中选择“钥匙”或“锁”，主列表、快速粘贴和编辑类型保持
一致。若字体暂时没有合适字形，首期可复用纯文本图标，不能引入平台不一致的系统 emoji。

## 9. 历史数据与兼容性

### 9.1 新旧版本兼容

- 新版本写入的 `secret` 对旧版本是未知 `meta_type`。旧版本的
  `display_kind()` 会回退为普通文本，因此不会崩溃，但不会脱敏。
- 同步协议应继续透传未知 `meta_type` 字符串，不能用枚举反序列化拒绝新值。
- 新版本收到旧设备同步的空 `meta_type` 内容时，可执行同一检测器重新分类。
- 检测缓存和一次性回填记录必须包含 `SECRET_DETECTION_RULE_VERSION`；升级规则后旧缓存
  自动失效，是否再次回填由迁移策略显式决定。

这意味着混用旧版本时，旧设备仍可能显示原文。发布说明应明确：所有需要脱敏显示的设备
都必须升级。

### 9.2 历史数据回填

推荐提供一次性、保守的本地回填：

1. 只扫描 `content_type = 'plain_text' AND meta_type = ''` 的记录。
2. 只对高置信度规则自动写入 `meta_type = 'secret'`；疑似独立密码不回填。
3. 使用分页和事务批量更新，避免在 GPUI 主线程阻塞。
4. 不覆盖用户已手动选择的任何非空类型。
5. 不记录原文、前后缀或匹配值日志，只记录扫描数量、命中数量和错误数量。
6. 通过迁移版本标记保证只执行一次。
7. 回填标记同时记录 `SECRET_DETECTION_RULE_VERSION`，规则升级不会静默重复扫描。
8. 更新后刷新列表和筛选状态；是否更新 `updated_at`、生成同步脏标记需与现有同步冲突
   规则保持一致。

如果版本排期不允许做安全的后台回填，首期可以只处理新复制和手动标记的内容，但必须在
发布说明中注明“已有历史记录不会自动重新识别”。

## 10. 性能与安全约束

- 检测器在 200ms 统一轮询链路中执行，规则应为线性扫描或预编译匹配，不能每次复制都
  重复编译正则。
- 为待检测文本设置合理上限；超大文本只执行明确字段和私钥块等线性规则，跳过熵检测。
- 不进行网络请求，不验证密钥是否真实有效。
- 使用标准库 `std::sync::LazyLock` 初始化静态 `Regex`/`RegexSet`。`RegexSet` 只用于
  快速判断候选规则，实际 `value_ranges` 仍由带捕获组的 `Regex` 或结构化解析器生成；
  不能把 `RegexSet` 当作范围提取器。
- 任何错误日志只能包含规则名称、文本长度和错误类别，禁止打印原文或脱敏前后缀。
- 固定四个星号，避免通过掩码长度推断真实长度。
- UI、数据库和同步继续保存原文的事实必须在隐私说明中明确。
- GPUI 渲染只使用检测结果，不在 `render()` 内进行高成本解析；结果可在卡片构建或小型
  纯函数中计算并按内容哈希缓存。

## 11. 测试方案

### 11.1 单元测试

1. **通用脱敏**

   - 8、9、长字符串均为前 3 + `****` + 后 4。
   - 1～7 字符只显示 `****`。
   - 中文、emoji、组合字符不 panic、不破坏 UTF-8。
   - 掩码固定长度，不随原文变化。

2. **识别正例**

   - 各厂商前缀和合法长度。
   - `password=...`、JSON、YAML、带引号字段。
   - Bearer、Basic、JWT。
   - URL 内嵌密码。
   - URL 敏感 query 参数；条目仍为 `DisplayKind::Link`。
   - PEM 私钥块。
   - 多行配置中的多个敏感值。
   - 满足严格条件的独立疑似密码。

3. **识别负例**

   - 普通 URL、邮箱、电话、文件路径、颜色值、UUID。
   - 不含凭据的 URL 仍生成原有 host/path 预览和 favicon host。
   - Git commit SHA、常见内容哈希、订单号、版本号。
   - Stripe 公开 `pk_` Key。
   - 自然语言句子、Markdown、代码片段中仅出现 `password` 说明文字。
   - 过短随机字符串和重复字符。

4. **类型与筛选**

   - `meta_type = "secret"` 映射到 `DisplayKind::Secret`。
   - “信息”SQL 同时匹配 email、phone、secret。
   - 纯文本 SQL 排除 secret。
   - 编辑保存能正确往返 `secret`。
   - 未知 `meta_type` 仍安全回退为普通文本。
   - 已标记 `secret` 但当前规则无法重建范围时整段脱敏，不显示原文。
   - 相同输入在同一规则版本内始终返回相同、已排序且不重叠的范围。

5. **兼容与同步**

   - `secret` 经序列化、同步、反序列化后保持不变。
   - 历史回填只更新空类型、高置信度记录，且幂等。

测试数据只使用无效示例密钥，禁止把真实凭据提交到仓库。建议在
`src/core/secret.rs` 的独立 `test_vectors` 测试模块或
`tests/secret_test_vectors.rs` 中集中维护正例、负例和预期范围，避免厂商规则散落。

### 11.2 UI 验证

- 主列表和快速粘贴对同一条内容显示完全一致。
- 邮箱、电话现有样式没有意外回归。
- 密钥卡片显示“密钥”标签和统一图标。
- “信息”筛选能同时筛出邮箱、电话和密钥。
- 搜索命中隐藏部分时不显示原始字符。
- 带凭据或敏感 query 的 URL 仍显示为链接、保留 favicon/打开能力，但预览不出现凭据
  原文，favicon 请求也不包含 userinfo 或 query。
- 多行、多密钥、超长文本、窄窗口和不同卡片高度下不溢出。
- 明暗主题下掩码、选中态和搜索高亮均清晰。
- 复制、双击粘贴、快捷键粘贴仍输出完整原文。
- 编辑页能查看原文、改回文本并保存。

## 12. 验收标准

1. 复制高置信度 API Key、Token、显式密码字段或私钥后，条目被标记为“密钥”。
2. 单值密钥大于 7 个字符时只显示前 3、`****`、后 4；不大于 7 个字符时只显示
   `****`。
3. 主列表和快速粘贴不包含两套独立脱敏算法，也不再手工解析 `****`。
4. “信息”筛选返回邮箱、电话和密钥，普通“文本”筛选不重复返回这些语义子类型。
5. 原始复制和粘贴内容不被修改。
6. 用户可以在编辑面板手动选择或取消“密钥”类型。
7. 检测和错误日志中没有敏感原文。
8. 单元测试覆盖主要正例、负例、Unicode、短值、筛选和同步兼容。
9. URL 内嵌凭据仍分类为 `link`，链接预览和 favicon 仅使用清洗后的 host/局部脱敏内容。
10. 检测器在同一规则版本内确定；无法重建已标记密钥的范围时整段脱敏。
11. `cargo test` 与 `cargo clippy --all-targets --all-features -- -D warnings`
   通过。

## 13. 实施顺序

1. 先建立 `core::secret`，实现规则版本、纯检测器、URL 安全解析、结构化脱敏 API 和
   集中测试向量。
2. 修正 URL host/favicon 清洗，确保 URL 始终保留 `link` 语义且预览不泄露凭据。
3. 接入 `DisplayKind::Secret`、非 URL 剪贴板分类、编辑类型、国际化和“信息”筛选。
4. 实现 `SensitiveText`，替换主列表与快速粘贴的现有脱敏分支。
5. 补充同步兼容测试、源码协议注释和 CHANGELOG。
6. 根据版本范围决定是否启用一次性历史回填。
7. 完成自动测试后，由用户运行现有应用实例进行人工 UI 验收。

## 14. 待产品确认

URL 与 Secret 的冲突已在本方案中确定：合法 URL 保持 `link`，只对 userinfo 密码和
敏感 query 局部脱敏，favicon 只使用 host，不再作为待确认项。

实施前建议只确认两个产品选择，其余可按本方案默认值执行：

1. 历史记录是否在首期自动回填；推荐只回填高置信度、当前无语义类型的纯文本。
2. 短邮箱本地部分是否按安全规则进一步隐藏；推荐修正，避免 3 个字符以内的邮箱本地
   部分完整暴露。
