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

pub fn detect_secret(text: &str) -> Option<SecretMatch>;
```

`value_ranges` 指向真正需要隐藏的值，而不是一律隐藏整段文本。例如
`API_KEY=sk-proj-xxxx` 应保留 `API_KEY=`，只脱敏等号后的值。范围只在内存中使用，
不写入数据库；展示时对原文重新执行同一个确定性检测器。

范围使用 UTF-8 字节边界存储，但必须由 `char_indices()` 生成并验证边界，禁止直接按
任意字节截断。

### 5.2 识别优先级

剪贴板文本检测顺序建议调整为：

```text
Files
→ Image
→ Secret
→ Link
→ Path
→ Color
→ Email / Phone
→ RichText
→ Markdown
→ PlainText
```

密钥检测放在链接、路径和颜色之前，是为了正确处理带认证信息的 URL、Bearer Token、
`.env` 配置和以特殊前缀开头的令牌。密钥规则必须足够保守，避免普通链接或路径被抢先
分类。

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

   对形如 `scheme://user:password@host` 的文本，仅脱敏密码部分。普通不含凭据的 URL
   仍分类为链接。

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

若文本命中私钥块等无法安全局部展示的格式，可以退化为整段密钥摘要预览。

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

如果快速粘贴当前结构难以直接复用完整组件，至少必须复用
`sensitive_preview_parts`，并在后续重构为同一组件；不应复制检测或掩码算法。

## 8. 代码影响范围

| 文件 | 主要改动 |
| --- | --- |
| `src/core/types.rs` | 新增 `DisplayKind::Secret`、密钥检测模型、`detect_secret`、通用脱敏结构和字符安全算法 |
| `src/platform/clipboard.rs` | 在文本检测链中加入密钥识别，写入 `meta_type = "secret"` |
| `src/core/filters.rs` | “信息”包含 `secret`，纯文本排除 `secret`，补充 SQL 测试 |
| `src/core/i18n_keys.rs` | 新增“密钥 / Secret”的编辑类型和卡片类型文案 |
| `src/ui/edit_panel.rs` | 类型选项、类型映射、纯文本编辑类型集合加入 `secret` |
| `src/state/app.rs` | 编辑保存映射加入 `("plain_text", "secret")` |
| `src/ui/components/sensitive_text.rs` | 新增统一敏感内容预览组件 |
| `src/ui/components/mod.rs` | 导出通用组件 |
| `src/ui/clipboard_card.rs` | 接入 `DisplayKind::Secret` 和统一组件，删除手工掩码拆分 |
| `src/ui/quick_paste.rs` | 接入密钥图标、类型和统一敏感预览 |
| `src/core/sync.rs` | 更新 `meta_type` 协议注释和兼容性测试；确认 `secret` 原样透传 |
| `src/core/migration.rs` / DB 层 | 可选的一次性历史数据回填 |

密钥图标优先从现有内嵌 icon font 中选择“钥匙”或“锁”，主列表、快速粘贴和编辑类型保持
一致。若字体暂时没有合适字形，首期可复用纯文本图标，不能引入平台不一致的系统 emoji。

## 9. 历史数据与兼容性

### 9.1 新旧版本兼容

- 新版本写入的 `secret` 对旧版本是未知 `meta_type`。旧版本的
  `display_kind()` 会回退为普通文本，因此不会崩溃，但不会脱敏。
- 同步协议应继续透传未知 `meta_type` 字符串，不能用枚举反序列化拒绝新值。
- 新版本收到旧设备同步的空 `meta_type` 内容时，可执行同一检测器重新分类。

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
7. 更新后刷新列表和筛选状态；是否更新 `updated_at`、生成同步脏标记需与现有同步冲突
   规则保持一致。

如果版本排期不允许做安全的后台回填，首期可以只处理新复制和手动标记的内容，但必须在
发布说明中注明“已有历史记录不会自动重新识别”。

## 10. 性能与安全约束

- 检测器在 200ms 统一轮询链路中执行，规则应为线性扫描或预编译匹配，不能每次复制都
  重复编译正则。
- 为待检测文本设置合理上限；超大文本只执行明确字段和私钥块等线性规则，跳过熵检测。
- 不进行网络请求，不验证密钥是否真实有效。
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
   - PEM 私钥块。
   - 多行配置中的多个敏感值。
   - 满足严格条件的独立疑似密码。

3. **识别负例**

   - 普通 URL、邮箱、电话、文件路径、颜色值、UUID。
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

5. **兼容与同步**

   - `secret` 经序列化、同步、反序列化后保持不变。
   - 历史回填只更新空类型、高置信度记录，且幂等。

测试数据只使用无效示例密钥，禁止把真实凭据提交到仓库。

### 11.2 UI 验证

- 主列表和快速粘贴对同一条内容显示完全一致。
- 邮箱、电话现有样式没有意外回归。
- 密钥卡片显示“密钥”标签和统一图标。
- “信息”筛选能同时筛出邮箱、电话和密钥。
- 搜索命中隐藏部分时不显示原始字符。
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
9. `cargo test` 与 `cargo clippy --all-targets --all-features -- -D warnings`
   通过。

## 13. 实施顺序

1. 先实现 `DisplayKind::Secret`、检测器、结构化脱敏 API 和单元测试。
2. 接入剪贴板分类、编辑类型、国际化和“信息”筛选。
3. 实现 `SensitiveText`，替换主列表与快速粘贴的现有脱敏分支。
4. 补充同步兼容测试和源码协议注释。
5. 根据版本范围决定是否启用一次性历史回填。
6. 完成自动测试后，由用户运行现有应用实例进行人工 UI 验收。

## 14. 待产品确认

实施前建议只确认两个产品选择，其余可按本方案默认值执行：

1. 历史记录是否在首期自动回填；推荐只回填高置信度、当前无语义类型的纯文本。
2. 短邮箱本地部分是否按安全规则进一步隐藏；推荐修正，避免 3 个字符以内的邮箱本地
   部分完整暴露。
