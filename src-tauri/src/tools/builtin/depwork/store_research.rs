//! store_research — 店铺调研工具集 (Depwork only).
//!
//! 店铺调研管线(市场经理)的数据采集层:给 agent 三个入口工具,
//! 复用浏览器接管会话(专用 profile、登录态跨会话持久):
//!
//! - `store_research_map` — 高德地图定位:搜索店铺,页面文本含 POI 卡片
//!   (店名/地址/电话/评分/人均/营业时间/标签),之后可用 browser_control
//!   缩放/拖动/截图接力看周边
//! - `store_research_xhs` — 小红书口碑:搜索 城市+品类 或 店名,
//!   页面文本含笔记标题/作者/点赞数
//! - `store_research_geo` — 高德官方 API:定位 + 周边 POI(纯 HTTP 不受风控)
//!
//! 工具只负责"导航到对的入口 + 等待渲染 + 返回页面文本",由模型自己从
//! 文本中提炼结论;细节(看周边地图、翻页、截图分析)用 browser_control
//! 和 visual_describe 接力。小红书需要登录时返回指引,由模型决定 handoff
//! 让用户登录一次。

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::browser::session_profile_key;
use crate::browser::{BrowserManager, LaunchOptions};
use crate::core::error::AppResult;
use crate::bootstrap::AppState;
use async_trait::async_trait;
use serde_json::{json, Value};

/// 常用城市名 → 高德 adcode。高德网页搜索传 `city` 限定城市,
/// 不传则按访问 IP 定位。仅收录高频城市,其它城市可不传 city。
fn adcode_of(city: &str) -> Option<&'static str> {
    let c = city.trim();
    let table: &[(&str, &str)] = &[
        ("北京", "110000"),
        ("上海", "310000"),
        ("广州", "440100"),
        ("深圳", "440300"),
        ("杭州", "330100"),
        ("成都", "510100"),
        ("重庆", "500000"),
        ("武汉", "420100"),
        ("西安", "610100"),
        ("南京", "320100"),
        ("天津", "120000"),
        ("苏州", "320500"),
        ("长沙", "430100"),
        ("郑州", "410100"),
        ("青岛", "370200"),
        ("沈阳", "210100"),
        ("大连", "210200"),
        ("昆明", "530100"),
        ("厦门", "350200"),
        ("福州", "350100"),
        ("合肥", "340100"),
        ("济南", "370100"),
        ("石家庄", "130100"),
        ("南昌", "360100"),
        ("贵阳", "520100"),
        ("南宁", "450100"),
        ("太原", "140100"),
        ("哈尔滨", "230100"),
        ("长春", "220100"),
        ("呼和浩特", "150100"),
        ("乌鲁木齐", "650100"),
        ("兰州", "620100"),
        ("银川", "640100"),
        ("西宁", "630100"),
        ("海口", "460100"),
        ("三亚", "460200"),
        ("无锡", "320200"),
        ("宁波", "330200"),
        ("温州", "330300"),
        ("佛山", "440600"),
        ("东莞", "441900"),
        ("珠海", "440400"),
        ("泉州", "350500"),
        ("烟台", "370600"),
        ("徐州", "320300"),
        ("常州", "320400"),
        ("南通", "320600"),
        ("绍兴", "330600"),
        ("嘉兴", "330400"),
        ("台州", "331000"),
        ("洛阳", "410300"),
        ("襄阳", "420600"),
        ("宜昌", "420500"),
        ("岳阳", "430600"),
        ("柳州", "450200"),
        ("桂林", "450300"),
        ("绵阳", "510700"),
        ("遵义", "520300"),
        ("大理", "532900"),
        ("丽江", "530700"),
        ("拉萨", "540100"),
        ("秦皇岛", "130300"),
        ("唐山", "130200"),
        ("保定", "130600"),
        ("廊坊", "131000"),
        ("邯郸", "130400"),
        ("沧州", "130900"),
    ];
    table
        .iter()
        .find(|(name, _)| *name == c)
        .map(|(_, code)| *code)
}

/// Fetch the shared browser manager from the tool context.
fn manager(context: &ToolContext) -> AppResult<std::sync::Arc<BrowserManager>> {
    use tauri::Manager as _;
    let state = context.app.state::<AppState>();
    Ok(state.browser.clone())
}

/// This conversation's isolated browser profile key — shared with
/// `browser_control` so both tools drive the SAME browser session.
fn profile_key(context: &ToolContext) -> String {
    session_profile_key(&context.session_id)
}

/// 确保浏览器会话在跑:未运行才启动(已在运行绝不打扰用户/重复拉起)。
async fn ensure_browser(mgr: &BrowserManager, profile: &str) -> AppResult<()> {
    if !mgr.status_for(profile).await.running {
        mgr.start_for(
            None,
            LaunchOptions {
                profile: profile.to_string(),
                headless: false,
            },
        )
        .await?;
    }
    Ok(())
}

/// 轮询等待页面 JS 渲染稳定:可见文本连续两次采样一致(且非空)即视为
/// 渲染完成,15s 超时则按当前状态继续(尽力而为,不阻塞调研)。
async fn wait_render_stable(mgr: &BrowserManager, profile: &str, timeout_secs: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut prev: Option<usize> = None;
    while std::time::Instant::now() < deadline {
        match mgr.body_text_len_for(profile).await {
            Ok(len) if len > 200 && prev == Some(len) => return,
            Ok(len) => prev = Some(len),
            Err(_) => {}
        }
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    }
}

/// 打开搜索页 → 等 JS 渲染稳定 → 抓取页面文本。
/// 调研永远在**新标签**里进行,绝不劫持用户正在看的标签页;
/// 登录态走专用 profile 持久保存,登录一次后不再要求。
async fn search_and_read(
    mgr: &BrowserManager,
    profile: &str,
    url: &str,
    max_chars: usize,
) -> AppResult<String> {
    ensure_browser(mgr, profile).await?;
    if let Err(first) = mgr.tab_new_for(profile, url).await {
        // 僵尸会话:浏览器被手动关闭/崩溃但状态残留 → 重启后重试一次。
        tracing::warn!(error = %first, "tab_new failed — restarting browser session and retrying");
        let _ = mgr.stop_for(profile).await;
        mgr.start_for(
            None,
            LaunchOptions {
                profile: profile.to_string(),
                headless: false,
            },
        )
        .await?;
        mgr.tab_new_for(profile, url).await?;
    }
    wait_render_stable(mgr, profile, 15).await;
    mgr.read_page_for(profile, max_chars).await
}

/// 调研标签保留提示——多次调研会累积标签,提醒模型用完可关闭。
const TAB_HINT: &str =
    "\n\n[提示] 调研标签保留在浏览器里,全部用完可用 browser_control tab_close 关闭。";

/// 尽力截图当前页面并返回落盘路径(失败返回 None)——调研每步留痕,
/// 模型可对路径调 visual_describe 看图。
async fn shot(context: &ToolContext, mgr: &BrowserManager, profile: &str) -> Option<String> {
    match mgr.screenshot_for(profile).await {
        Ok(png) => crate::tools::builtin::depwork::browser_control::save_screenshot(context, &png)
            .await
            .ok(),
        Err(_) => None,
    }
}

/// 组装调研结果:页面文本 + 截图提示 + 运营诊断视角。
async fn finish(
    page: &str,
    context: &ToolContext,
    mgr: &BrowserManager,
    profile: &str,
    view: &str,
) -> String {
    let mut out = String::new();
    if let Some(path) = shot(context, mgr, profile).await {
        out.push_str(&format!(
            "[截图] {path} — 可调 visual_describe 查看当前页面\n\n"
        ));
    }
    out.push_str(page);
    out.push_str("\n\n[调研视角] ");
    out.push_str(view);
    out
}

/// 组装高德搜索 URL:query 必带,城市名映射到 adcode 时附 city 参数。
fn amap_search_url(store: &str, city: &str) -> String {
    let mut url = format!(
        "https://www.amap.com/search?query={}",
        urlencoding::encode(store.trim())
    );
    if !city.trim().is_empty() {
        if let Some(code) = adcode_of(city) {
            url.push_str(&format!("&city={code}"));
        }
    }
    url
}

/// 通用只读声明:调研工具只浏览页面、不改任何东西。
fn shared_read_only() -> bool {
    true
}

fn shared_concurrency() -> bool {
    false
}

// ── 1. store_research_map (高德定位) ─────────────────────────────────

pub struct StoreResearchMapTool;

impl StoreResearchMapTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for StoreResearchMapTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "store_research_map"
    }

    fn description(&self) -> &str {
        "高德地图定位店铺(店铺调研第一步):输入店铺名(可带城市),自动在浏览器中打开高德搜索, \
         返回 POI 列表文本 — 店名/地址/电话/评分/人均/营业时间/品类标签。用于确认目标店铺的 \
         准确位置和基础信息。结果里若有多个同名店,用地址/评分判断哪家是目标。\
         之后可用 browser_control click 点进 POI 详情、read_page 看更多。参数: \
         store (必填,店铺名,可加品类如\"金原火锅\"), city (可选,城市名,如\"北京\", \
         不传按 IP 定位), max_chars (可选,返回文本上限,默认 12000)。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "store": {
                    "type": "string",
                    "description": "店铺名,如\"金原火锅\"、\"老王牛肉面\"。"
                },
                "city": {
                    "type": "string",
                    "description": "城市名(可选),如\"北京\"、\"杭州\"。不传则按 IP 定位城市。"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "返回页面文本上限,默认 12000。"
                }
            },
            "required": ["store"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_read_only_call(&self, _args: &Value) -> bool {
        shared_read_only()
    }

    fn is_concurrency_safe(&self) -> bool {
        shared_concurrency()
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let store = args
            .get("store")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: store".to_string())?
            .to_string();
        let city = args
            .get("city")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .unwrap_or(12_000)
            .clamp(2000, 40_000) as usize;
        let url = amap_search_url(&store, &city);
        let mgr = manager(context)?;
        let profile = profile_key(context);
        let page = search_and_read(&mgr, &profile, &url, max_chars).await?;
        let city_note = if !city.trim().is_empty() && adcode_of(&city).is_none() {
            format!(
                "\n[注意] 城市「{city}」未收录 adcode 表,已按 IP 定位搜索;如定位不准,可让用户在高德页面内切换城市。"
            )
        } else {
            String::new()
        };
        let view = "POI 卡片要点→运营方案:\n\
             - 地址/商圈:线下曝光与商圈竞争密度判断\n\
             - 评分/人均:当前口碑起点,与高德周边截图/小红书口碑交叉验证\n\
             - 营业时间:运营策略窗口(午市/晚市/夜宵档)\n\
             - 若列表有多个同名店,用地址/评分确认目标;进详情可用 browser_control click(店名)+ read_page。";
        let risk = risk_hint(&page).unwrap_or("");
        let out = format!(
            "{}\n\n[调研提示] 从上方文本里找出目标店铺的 POI 卡片(店名/地址/电话/评分/营业时间)。\
             若页面提示登录/验证码或结果为空,用 browser_control handoff 让用户处理; \
             结果多时可 scroll 后再 read_page。{}{}{}",
            finish(&page, context, &mgr, &profile, view).await,
            city_note,
            risk,
            TAB_HINT
        );
        Ok(ToolResult::success(out))
    }
}

// ── 3. store_research_xhs (小红书口碑) ───────────────────────────────

pub struct StoreResearchXhsTool;

impl StoreResearchXhsTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for StoreResearchXhsTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "store_research_xhs"
    }

    fn description(&self) -> &str {
        "小红书口碑调研(店铺调研第三步):输入关键词(城市+品类 或 店铺名,如\"北京 火锅\"), \
         自动在浏览器中打开小红书搜索,返回笔记列表文本 — 标题/作者/点赞数/摘要。\
         用于评估店铺在种草平台的声量、发现可复用的内容方向。\
         参数: keyword (必填,如\"北京 火锅\"、\"金原火锅 探店\"), max_chars (可选,默认 12000)。\
         小红书网页版需要登录:首次搜索若提示登录,用 browser_control handoff \
         让用户登录一次(登录态持久,之后直接可用)。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "keyword": {
                    "type": "string",
                    "description": "搜索词,如\"北京 火锅\"、\"金原火锅 探店\"、\"城市+品类\"。"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "返回页面文本上限,默认 12000。"
                }
            },
            "required": ["keyword"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_read_only_call(&self, _args: &Value) -> bool {
        shared_read_only()
    }

    fn is_concurrency_safe(&self) -> bool {
        shared_concurrency()
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let keyword = args
            .get("keyword")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: keyword".to_string())?
            .to_string();
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .unwrap_or(12_000)
            .clamp(2000, 40_000) as usize;
        let url = format!(
            "https://www.xiaohongshu.com/search_result?keyword={}&source=web_explore_feed",
            urlencoding::encode(keyword.trim())
        );
        let mgr = manager(context)?;
        let profile = profile_key(context);
        let page = search_and_read(&mgr, &profile, &url, max_chars).await?;
        let view = "笔记要点→运营方案:\n\
             - 笔记数量/点赞:种草声量水位(远低于竞对 = 内容空白机会)\n\
             - 标题与内容方向:可复用的选题池\n\
             - 负面笔记:口碑风险点,需回复/整改。";
        let risk = risk_hint(&page).unwrap_or("");
        let out = format!(
            "{}\n\n[调研提示] 若页面提示登录,用 browser_control handoff 让用户登录一次即可。{}{}",
            finish(&page, context, &mgr, &profile, view).await,
            risk,
            TAB_HINT
        );
        Ok(ToolResult::success(out))
    }
}

// ── 4. store_research_geo (高德开放 API:定位 + 周边) ─────────────────

/// 浏览器工具页面文本的风控特征词——命中说明该平台 PC 网页被反爬拦截,
/// 硬闯无意义,给模型明确的降级路径。
const RISK_MARKERS: &[&str] = &[
    "验证码",
    "安全验证",
    "滑块",
    "滑动验证",
    "扫码登录",
    "扫码验证",
    "人机验证",
    "访问频繁",
    "请求过于频繁",
];

/// 检查页面文本是否命中风控特征,命中则返回降级指引,否则 None。
fn risk_hint(page_text: &str) -> Option<&'static str> {
    let hit = RISK_MARKERS.iter().any(|m| page_text.contains(m));
    hit.then_some(
        "该平台 PC 网页被风控拦截(验证码/扫码),继续硬闯没有意义。降级路径:\n\
         \x20 1) browser_control handoff 让用户扫码登录一次(登录态持久,之后可再用);\n\
         \x20 2) 或放弃该数据源——用 store_research_geo(高德 API,不受风控)拿店铺定位与周边数据, \
         \x20 评分/点评数据向用户确认。",
    )
}

/// 从高德 Web API 的 JSON 里提取 POI 列表文本(纯函数,便于测试)。
fn format_pois(pois: &[Value], limit: usize) -> String {
    let mut out = String::new();
    for p in pois.iter().take(limit) {
        let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let address = p.get("address").and_then(|v| v.as_str()).unwrap_or("");
        let tel = p.get("tel").and_then(|v| v.as_str()).unwrap_or("");
        let ty = p.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let loc = p.get("location").and_then(|v| v.as_str()).unwrap_or("");
        let pname = p.get("pname").and_then(|v| v.as_str()).unwrap_or("");
        let cityname = p.get("cityname").and_then(|v| v.as_str()).unwrap_or("");
        let adname = p.get("adname").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!("• {name} [{ty}]\n"));
        out.push_str(&format!("  地址: {pname}{cityname}{adname} {address}\n"));
        if !tel.is_empty() {
            out.push_str(&format!("  电话: {tel}\n"));
        }
        out.push_str(&format!("  坐标: {loc}\n"));
    }
    if pois.len() > limit {
        out.push_str(&format!("… 另有 {} 个结果未列出\n", pois.len() - limit));
    }
    out
}

/// 高德 API 店铺调研工具:官方开放平台 REST API(纯 HTTP,不受网页风控)。
pub struct StoreResearchGeoTool;

impl StoreResearchGeoTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for StoreResearchGeoTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Depwork
    }

    fn name(&self) -> &str {
        "store_research_geo"
    }

    fn description(&self) -> &str {
        "高德开放 API 店铺定位+周边调研(不受网页风控,纯 HTTP):输入店铺名+城市, \
         自动查询该店 POI(地址/电话/坐标/类型)并以店铺为中心查周边 radius 米内 \
         的竞对与配套(同品类店、餐饮/购物/酒店分布)——回答「这家店周边有什么」。\
         需要高德 Web 服务 API key:config.toml 的 [tools] amap_web_key 或环境变量 \
         AMAP_WEB_KEY(免费申请 https://console.amap.com/)。\
         参数: store (必填,店铺名), city (必填,城市名), radius (可选,周边半径米,默认 1500,最大 5000)。\
         注意:API 无评分/点评数据;评分类信息需配合高德地图浏览器调研或询问用户。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "store": {
                    "type": "string",
                    "description": "店铺名,如\"金原火锅\"。"
                },
                "city": {
                    "type": "string",
                    "description": "城市名(必填),如\"北京\"。API 需要城市编码,未收录城市会报错。"
                },
                "radius": {
                    "type": "integer",
                    "description": "周边搜索半径(米),默认 1500,范围 300-5000。"
                }
            },
            "required": ["store", "city"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn is_read_only_call(&self, _args: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let store = args
            .get("store")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: store".to_string())?
            .to_string();
        let city = args
            .get("city")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: city".to_string())?
            .to_string();
        let radius = args
            .get("radius")
            .and_then(|v| v.as_u64())
            .unwrap_or(1500)
            .clamp(300, 5000);

        use tauri::Manager as _;
        let state = context.app.state::<AppState>();
        let key = {
            // Poison-recovery pattern matches the rest of the codebase —
            // a panicked writer must never crash the tool call.
            let cfg = state.config.read().unwrap_or_else(|e| e.into_inner());
            cfg.tools.amap_web_key.clone()
        };
        if key.trim().is_empty() {
            return Err(
                "未配置高德 API key:请在 ~/.deepdepcat/config.toml 的 [tools] 段加 \
                 amap_web_key = \"你的key\"(免费申请 https://console.amap.com/),\
                 或设置环境变量 AMAP_WEB_KEY。配置后重试。"
                    .to_string()
                    .into(),
            );
        }
        let code = adcode_of(&city).ok_or_else(|| {
            format!("城市「{city}」未收录,store_research_geo 需要城市编码(前 67 高频城市表)")
        })?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| format!("HTTP client error: {e}"))?;

        // 1. place/text — 找目标店铺
        let text_url = format!(
            "https://restapi.amap.com/v3/place/text?key={}&keywords={}&city={}&offset=5&extensions=base",
            key,
            urlencoding::encode(store.trim()),
            code
        );
        let text_resp: Value = client
            .get(&text_url)
            .send()
            .await
            .map_err(|e| format!("高德 place/text 请求失败: {e}"))?
            .json()
            .await
            .map_err(|e| format!("高德 place/text 响应解析失败: {e}"))?;
        let status = text_resp
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("0");
        let info = text_resp
            .get("info")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if status != "1" {
            return Err(format!("高德 place/text 返回错误: {info}").into());
        }
        let pois = text_resp
            .get("pois")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let target = pois
            .first()
            .ok_or_else(|| format!("在「{city}」没搜到「{store}」相关 POI,换个店名或城市重试"))?;
        let target_loc = target
            .get("location")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let target_name = target
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(store.as_str())
            .to_string();
        if target_loc.is_empty() {
            return Err(format!("POI「{target_name}」缺少坐标,无法查周边").into());
        }

        // 2. place/around — 周边竞对与配套
        let around_url = format!(
            "https://restapi.amap.com/v3/place/around?key={}&location={}&radius={}&offset=25&sortrule=distance&extensions=base",
            key, target_loc, radius
        );
        let around_resp: Value = client
            .get(&around_url)
            .send()
            .await
            .map_err(|e| format!("高德 place/around 请求失败: {e}"))?
            .json()
            .await
            .map_err(|e| format!("高德 place/around 响应解析失败: {e}"))?;
        let status = around_resp
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("0");
        if status != "1" {
            let around_info = around_resp
                .get("info")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Err(format!("高德 place/around 返回错误: {around_info}").into());
        }
        let around = around_resp
            .get("pois")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        // 3. 汇总:目标店 + 周边统计(按一级类型分组)
        let mut by_cat: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for p in &around {
            let ty = p
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("其它")
                .to_string();
            let cat = ty.split(';').next().unwrap_or("其它").to_string();
            *by_cat.entry(cat).or_insert(0) += 1;
        }
        let cat_summary: String = by_cat
            .iter()
            .map(|(k, v)| format!("{k}: {v} 个"))
            .collect::<Vec<_>>()
            .join("、");
        let view = format!(
            "调研视角→运营方案:\n\
             - 周边同品类密度:半径 {radius} 米内同类店数量直接决定竞争强度\n\
             - 配套类型(餐饮/购物/酒店/写字楼):推断客群与消费时段\n\
             - 目标店坐标可用于后续扩展(周边 5 公里供需分析)。\n\
             [数据源说明] 高德 API 无评分/点评/团购数据——评分与口碑需配合浏览器调研或向用户确认。"
        );
        let out = format!(
            "【目标店铺 POI】\n{}\n\n【周边 {radius} 米({} 个 POI,按距离排序)】\n{}\n\n【周边类型分布】\n{cat_summary}\n\n{view}",
            format_pois(std::slice::from_ref(target), 1),
            around.len(),
            format_pois(&around, 20),
        );
        Ok(ToolResult::success(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adcode_table_has_core_cities() {
        assert_eq!(adcode_of("北京"), Some("110000"));
        assert_eq!(adcode_of("上海"), Some("310000"));
        assert_eq!(adcode_of("三亚"), Some("460200"));
        assert_eq!(adcode_of("不存在城"), None);
        assert_eq!(adcode_of(""), None);
    }

    #[test]
    fn amap_url_encodes_store_and_city() {
        let url = amap_search_url("金原 火锅", "北京");
        assert_eq!(
            url,
            "https://www.amap.com/search?query=%E9%87%91%E5%8E%9F%20%E7%81%AB%E9%94%85&city=110000"
        );
        let url2 = amap_search_url("老王面馆", "");
        assert_eq!(
            url2,
            "https://www.amap.com/search?query=%E8%80%81%E7%8E%8B%E9%9D%A2%E9%A6%86"
        );
    }

    #[test]
    fn amap_url_unknown_city_omits_param() {
        let url = amap_search_url("测试店", "某小城");
        assert_eq!(
            url,
            "https://www.amap.com/search?query=%E6%B5%8B%E8%AF%95%E5%BA%97"
        );
    }

    #[test]
    fn xhs_url_encodes() {
        let url = format!(
            "https://www.xiaohongshu.com/search_result?keyword={}&source=web_explore_feed",
            urlencoding::encode("北京 火锅".trim())
        );
        assert_eq!(
            url,
            "https://www.xiaohongshu.com/search_result?keyword=%E5%8C%97%E4%BA%AC%20%E7%81%AB%E9%94%85&source=web_explore_feed"
        );
    }

    #[test]
    fn risk_hint_detects_captcha_walls() {
        assert!(risk_hint("欢迎光临,请完成安全验证后继续").is_some());
        assert!(risk_hint("扫码登录后查看完整内容").is_some());
        assert!(risk_hint("页面包含滑块验证").is_some());
        assert!(risk_hint("金原火锅 人均80元 评分4.5").is_none());
        assert!(risk_hint("").is_none());
    }

    #[test]
    fn format_pois_extracts_fields() {
        let pois = vec![json!({
            "name": "金原火锅",
            "address": "建国路88号",
            "tel": "010-12345678",
            "type": "餐饮服务;中餐厅;火锅店",
            "location": "116.4,39.9",
            "pname": "北京市",
            "cityname": "北京市",
            "adname": "朝阳区"
        })];
        let out = format_pois(&pois, 5);
        assert!(out.contains("金原火锅"));
        assert!(out.contains("建国路88号"));
        assert!(out.contains("010-12345678"));
        assert!(out.contains("116.4,39.9"));
        assert!(out.contains("朝阳区"));
    }

    #[test]
    fn format_pois_truncates_beyond_limit() {
        let pois: Vec<Value> = (0..10)
            .map(|i| json!({ "name": format!("店{i}"), "location": "1,2" }))
            .collect();
        let out = format_pois(&pois, 3);
        assert!(out.contains("另有 7 个结果未列出"));
        assert!(!out.contains("店5"));
    }

    #[test]
    fn format_pois_handles_empty() {
        assert!(format_pois(&[], 5).is_empty());
    }
}
