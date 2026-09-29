//! Model Context Protocol endpoint (`POST /mcp`, Streamable HTTP transport, JSON responses).
//!
//! Lets AI agents do what a person does on the site: search and browse, download, upload,
//! comment and rate, 求资料, and for reviewers / admins the review and admin pages. With a
//! personal access token (`Authorization: Bearer xmh_…`, created on the 「我的」 page) the
//! agent acts as the token's owner, with the owner's role.
//!
//! Besides the dedicated tools, `call_api` runs any page's `/api` request in-process, through
//! the same router, permission checks and rate limits as the browser. File bytes never come
//! here: like the browser, the agent sends upload parts to the target the plan names and
//! downloads from the mirror links (AGENTS.md §1).

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tower::ServiceExt;

use xmuhub_core::hub::{SearchItem, Viewer};
use xmuhub_core::model::{Id, Tag};
use xmuhub_core::search::{DocType, Filter};

use crate::api::{App, Auth, CSRF_HEADER, client_ip, node_view, resource_view};

const SUPPORTED: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "鹭岛书阁是厦门大学学生资料共享站（往年试卷、笔记、课件、题库）。\
先用 search 找课程或资料（支持中文、课程代号和拼音首字母，如 wjf=微积分），\
用 get_category 浏览课程下的资料，用 get_download_links 取下载地址（国内镜像，按速度排序，依次尝试即可）。\
资料 id 和分类 id 都是数字。写操作（上传、评论、评分、求资料、审核）需要个人令牌，令牌以其主人的身份和权限操作。\
下载：get_download_links 给出每个分卷的一组地址，用 curl 等工具依次尝试直到成功，按 sha256 校验，多个分卷按顺序拼接成一个文件。\
上传：先在本地把文件按 site_info 里的 limits.max_part 字节切成分卷、算出每卷的 sha256，调用 begin_upload；\
对每个 done=false 的分卷，按返回的 target 把该卷的原始字节作为请求体发过去（method、url、headers 原样使用，如 curl -X POST --data-binary @分卷文件 url），\
响应 JSON 里的 id 作为 asset_id 调用 confirm_upload_part；发送失败时用 renew_upload_part 换一个目标重试；\
所有分卷确认后调用 publish_upload 填写课程和类型（课程可以先用 suggest_category 查，查不到可以不填，会放进「待整理」）。\
网页上能做的其他操作（改资料、删除申请、提问、友情链接、管理员页面等）都可以用 call_api 调用对应的 /api 接口，接口列表见 list_api。";

fn tools(max_part: u64, max_file: u64) -> Value {
    let id = |d: &str| json!({ "type": "integer", "description": d });
    let text = |d: &str| json!({ "type": "string", "description": d });
    let resource_fields = json!({
        "node": id("课程/分类 id（search 或 suggest_category 查到的；publish_upload 可省略，放进「待整理」）"),
        "type_word": text("资料类型，如 期末试卷、期中试卷、复习提纲、笔记、课件、题库、实验报告（完整列表见 site_info 的 type_words）"),
        "time": text("学年学期或年份，如 2023-2024秋、2025春、2023；不确定可留空"),
        "paper": { "type": "string", "enum": ["", "A卷", "B卷", "C卷"] },
        "with_answer": { "type": "boolean", "description": "含答案" },
        "extra": text("文件名里的补充说明（短）"),
        "note": text("给其他同学看的备注（老师、范围等）"),
        "subtitle": text("内容说明（选填）"),
        "major": text("适用专业（同一门课各专业内容不同时填写）"),
    });
    // Split in two: one json! literal this long exceeds the macro recursion limit.
    let mut list = json!([
        { "name": "search", "description": "搜索课程/分类和资料。返回分类（type=node）和资料（type=resource）两类结果。",
          "inputSchema": { "type": "object", "properties": {
              "query": { "type": "string", "description": "关键词：课程名、代号、年份、类型或拼音首字母" },
              "type": { "type": "string", "enum": ["all", "node", "resource"], "description": "默认 all" },
              "tag": { "type": "string", "description": "资料标签 T1–T9（T1 真题试卷, T2 答案, T3 提纲笔记, T4 题库, T5 课件, T6 实验, T7 合集, T8 电子书, T9 工具模板）" },
              "within": id("只在该分类 id 的子树中搜索"),
              "page": { "type": "integer", "minimum": 1, "description": "每页 20 条" } },
            "required": ["query"] } },
        { "name": "get_tree", "description": "列出某分类的直接下级（不给 parent 则列出顶层栏目），含 id 和已发布资料数。",
          "inputSchema": { "type": "object", "properties": { "parent": id("父分类 id，省略为顶层") } } },
        { "name": "get_category", "description": "某个分类/课程的信息、下级分类和其中的全部资料。",
          "inputSchema": { "type": "object", "properties": { "id": id("分类 id") }, "required": ["id"] } },
        { "name": "get_resource", "description": "一份资料的详细信息（文件名、课程、时间、类型、大小、下载次数、评分）。",
          "inputSchema": { "type": "object", "properties": { "id": id("资料 id") }, "required": ["id"] } },
        { "name": "get_download_links", "description": "资料的下载地址：每个分卷一组 URL（国内镜像按速度排序，最后一个是 GitHub 直链），依次尝试即可。计一次下载。",
          "inputSchema": { "type": "object", "properties": { "id": id("资料 id") }, "required": ["id"] } },
        { "name": "list_recent", "description": "最新发布的资料。",
          "inputSchema": { "type": "object", "properties": { "limit": { "type": "integer", "minimum": 1, "maximum": 50 } } } },
        { "name": "list_popular", "description": "下载最多的资料。",
          "inputSchema": { "type": "object", "properties": { "limit": { "type": "integer", "minimum": 1, "maximum": 50 } } } },
        { "name": "get_comments", "description": "资料的评分汇总和评论。",
          "inputSchema": { "type": "object", "properties": { "id": id("资料 id") }, "required": ["id"] } },
        { "name": "post_comment", "description": "以令牌主人身份发表评论（需要令牌）。",
          "inputSchema": { "type": "object", "properties": { "id": id("资料 id"), "body": { "type": "string", "maxLength": 500 } }, "required": ["id", "body"] } },
        { "name": "rate_resource", "description": "给资料打 1–5 星，0 表示取消（需要令牌）。",
          "inputSchema": { "type": "object", "properties": { "id": id("资料 id"), "stars": { "type": "integer", "minimum": 0, "maximum": 5 } }, "required": ["id", "stars"] } },
        { "name": "whoami", "description": "当前令牌对应的账号和角色（无令牌时为访客）。",
          "inputSchema": { "type": "object", "properties": {} } },
        { "name": "submit_feedback", "description": "向站点管理员提交意见反馈。",
          "inputSchema": { "type": "object", "properties": { "body": { "type": "string" }, "contact": { "type": "string" } }, "required": ["body"] } },
        { "name": "review_queue", "description": "审核员：待审核/待核实/仅内部的资料列表。",
          "inputSchema": { "type": "object", "properties": {
              "status": { "type": "string", "enum": ["pending", "restricted"], "description": "省略为待审核+待复核" },
              "uncertain_only": { "type": "boolean" } } } },
        { "name": "review_resource", "description": "审核员：通过/驳回/下架/设为仅内部/恢复一份资料。",
          "inputSchema": { "type": "object", "properties": { "id": id("资料 id"),
              "action": { "type": "string", "enum": ["approve", "reject", "remove", "restrict", "restore"] },
              "note": { "type": "string" } }, "required": ["id", "action"] } },
    ]);
    let more = json!([
        { "name": "site_info", "description": "站点信息：公告、资料类型（type_words）、标签、上传大小限制（limits.max_file / max_part）、统计。",
          "inputSchema": { "type": "object", "properties": {} } },
        { "name": "suggest_category", "description": "按课程名/代号/拼音首字母查找课程（含所在路径），上传前用来确定 node。",
          "inputSchema": { "type": "object", "properties": { "q": text("关键词") }, "required": ["q"] } },
        { "name": "create_category", "description": "新建课程（要由另一位审核员确认，审核员自己建的也一样，管理员免审；需要令牌）。",
          "inputSchema": { "type": "object", "properties": { "parent": id("上级分类 id（学院/栏目）"), "name": text("课程名"),
              "kind": { "type": "string", "enum": ["course", "group", "section"], "description": "默认 course" },
              "level": { "type": "integer", "enum": [0, 1, 2, 3], "description": "0 不限 1 本科 2 研究生 3 本研" } },
            "required": ["parent", "name"] } },
        { "name": "begin_upload", "description": format!("开始上传一个文件（需要令牌）。先在本地把文件按 {max_part} 字节切成分卷（不到这个大小就只有一卷），算出每卷的 sha256；单个文件最大 {max_file} 字节。返回 upload_id 和每卷的发送目标 target；dedup=true 表示站上已有相同的文件，不用发送，直接 publish_upload。"),
          "inputSchema": { "type": "object", "properties": {
              "filename": text("原文件名（含扩展名）"), "mime": text("MIME 类型（选填）"),
              "parts": { "type": "array", "items": { "type": "object", "properties": { "size": { "type": "integer" }, "sha256": { "type": "string" } }, "required": ["size", "sha256"] } } },
            "required": ["filename", "parts"] } },
        { "name": "get_upload", "description": "上传进度：哪些分卷已确认，没确认的分卷的新发送目标（断点续传用）。",
          "inputSchema": { "type": "object", "properties": { "upload_id": id("begin_upload 返回的 upload_id") }, "required": ["upload_id"] } },
        { "name": "confirm_upload_part", "description": "一个分卷发送成功后调用；asset_id 是发送目标返回的 JSON 里的 id。",
          "inputSchema": { "type": "object", "properties": { "upload_id": id("upload_id"), "index": { "type": "integer" }, "asset_id": { "type": "integer" } }, "required": ["upload_id", "index"] } },
        { "name": "renew_upload_part", "description": "分卷发送失败时换一个发送目标重试；via_relay=true 表示上传节点连不上，改走本站中转。",
          "inputSchema": { "type": "object", "properties": { "upload_id": id("upload_id"), "index": { "type": "integer" }, "via_relay": { "type": "boolean" } }, "required": ["upload_id", "index"] } },
        { "name": "preview_name", "description": "按填写的信息预览资料在站上的文件名（不提交）。",
          "inputSchema": { "type": "object", "properties": { "ext": text("扩展名，如 pdf"), "fields": { "type": "object", "description": "和 publish_upload 的 fields 相同" } }, "required": ["fields"] } },
        { "name": "publish_upload", "description": "所有分卷确认后提交资料（需要令牌）。可信贡献者直接发布；其他人（包括审核员、管理员）要等另一位审核员通过。",
          "inputSchema": { "type": "object", "properties": { "upload_id": id("upload_id"), "fields": { "type": "object", "properties": resource_fields.clone(), "required": ["type_word"] } }, "required": ["upload_id", "fields"] } },
        { "name": "my_uploads", "description": "我上传的资料（含待审核的、未通过的原因），分页。",
          "inputSchema": { "type": "object", "properties": { "q": text("按标题/课程/原文件名筛选"),
              "status": { "type": "string", "enum": ["", "published", "pending", "rejected", "removed", "restricted"] },
              "offset": { "type": "integer" }, "limit": { "type": "integer", "maximum": 100 } } } },
        { "name": "edit_resource", "description": "修改资料信息（上传者在审核前，或审核员）。fields 要给全：先 get_resource 拿到当前值再改。",
          "inputSchema": { "type": "object", "properties": { "id": id("资料 id"), "fields": { "type": "object", "properties": resource_fields, "required": ["node", "type_word"] } }, "required": ["id", "fields"] } },
        { "name": "request_resource_change", "description": "上传者对已发布的资料提申请：kind=note 改备注（value 为新备注），kind=delete 申请下架（value 写原因）。",
          "inputSchema": { "type": "object", "properties": { "id": id("资料 id"), "kind": { "type": "string", "enum": ["note", "delete"] }, "value": { "type": "string" } }, "required": ["id", "kind"] } },
        { "name": "report_resource", "description": "举报资料（内容错误、侵权、无关等）。",
          "inputSchema": { "type": "object", "properties": { "id": id("资料 id"), "reason": { "type": "string" } }, "required": ["id", "reason"] } },
        { "name": "move_resources", "description": "审核员：把多份资料移到另一门课程（文件名随之更新）。",
          "inputSchema": { "type": "object", "properties": { "ids": { "type": "array", "items": { "type": "integer" } }, "node": id("目标课程 id") }, "required": ["ids", "node"] } },
        { "name": "list_wants", "description": "求资料帖：status=open 求助中（默认）、found 已找到、mine 我发的、pending 待审核（审核员）。",
          "inputSchema": { "type": "object", "properties": { "status": { "type": "string", "enum": ["open", "found", "mine", "pending"] }, "node": id("只看某门课") } } },
        { "name": "get_want", "description": "一条求资料帖和它的回复。",
          "inputSchema": { "type": "object", "properties": { "id": id("帖子 id") }, "required": ["id"] } },
        { "name": "post_want", "description": "发求资料帖（审核通过后公开；需要令牌）。",
          "inputSchema": { "type": "object", "properties": { "title": { "type": "string", "maxLength": 60 }, "body": { "type": "string", "maxLength": 500 }, "node": id("哪门课（选填）") }, "required": ["title"] } },
        { "name": "reply_want", "description": "回复求资料帖，可以附上本站资料 id。",
          "inputSchema": { "type": "object", "properties": { "id": id("帖子 id"), "body": { "type": "string", "maxLength": 500 }, "resource": id("资料 id（选填）") }, "required": ["id"] } },
        { "name": "vote_want", "description": "「我也要」：on=true 加上，false 取消。",
          "inputSchema": { "type": "object", "properties": { "id": id("帖子 id"), "on": { "type": "boolean" } }, "required": ["id", "on"] } },
        { "name": "set_want_status", "description": "发帖人或审核员：标记已找到（可附资料 id）、关闭或重新打开。",
          "inputSchema": { "type": "object", "properties": { "id": id("帖子 id"), "status": { "type": "string", "enum": ["found", "closed", "open"] }, "resource": id("找到的资料 id") }, "required": ["id", "status"] } },
        { "name": "review_want", "description": "审核员：通过或驳回（要写原因）一条求资料帖。",
          "inputSchema": { "type": "object", "properties": { "id": id("帖子 id"), "approve": { "type": "boolean" }, "note": { "type": "string" } }, "required": ["id", "approve"] } },
        { "name": "site_stats", "description": "公开统计：资料数、课程数、总下载量，下载最多/资料最多/近 30 天新增最多的课程，每周新增。",
          "inputSchema": { "type": "object", "properties": {} } },
        { "name": "follow_course", "description": "关注（on=true）或取消关注一门课程/学院；关注后有新资料审核通过会收到站内提醒（需要令牌）。",
          "inputSchema": { "type": "object", "properties": { "id": id("课程/学院 id"), "on": { "type": "boolean" } }, "required": ["id", "on"] } },
        { "name": "list_notices", "description": "我的站内提醒（新资料、审核结果、求资料回复等）和未读数；mark_read=true 同时全部标为已读。",
          "inputSchema": { "type": "object", "properties": { "limit": { "type": "integer", "maximum": 200 }, "mark_read": { "type": "boolean" } } } },
        { "name": "favorite", "description": "把一份资料放进收藏夹（on=false 拿出）。不给 collection 时用「我的收藏」（没有就新建）。",
          "inputSchema": { "type": "object", "properties": { "resource": id("资料 id"), "collection": id("收藏夹 id（选填）"), "on": { "type": "boolean", "description": "默认 true" } }, "required": ["resource"] } },
        { "name": "list_api", "description": "网站 /api 接口列表（方法、路径、用途和参数），配合 call_api 使用。",
          "inputSchema": { "type": "object", "properties": {} } },
        { "name": "call_api", "description": "以令牌主人的身份调用网站的任意 /api 接口，权限检查和网页上操作完全一样。登录注册、令牌管理和传文件字节的接口除外。",
          "inputSchema": { "type": "object", "properties": {
              "method": { "type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE"] },
              "path": text("接口路径，如 /resources/123 或 /admin/users?q=abc（/api 前缀可省略）"),
              "query": { "type": "object", "description": "查询参数（选填）" },
              "body": { "description": "JSON 请求体（选填）" } },
            "required": ["method", "path"] } },
    ]);
    if let (Some(l), Value::Array(m)) = (list.as_array_mut(), more) {
        l.extend(m);
    }
    list
}

/// The `/api` routes for `list_api`: method, path, what it does and its parameters.
const API: &[(&str, &str, &str)] = &[
    ("GET", "/meta", "站点信息：公告、资料类型、标签、上传限制、统计"),
    ("GET", "/me", "当前账号"),
    ("PATCH", "/me", "改昵称/是否公开昵称 {nickname?, public_name?}"),
    ("POST", "/me/avatar", "设头像 {upload_id}（先按上传流程传一张 256×256 图片）"),
    ("DELETE", "/me/avatar", "删头像"),
    ("POST", "/inbox", "取得「待整理」分类（没有课程时上传到这里）"),
    ("GET", "/tree", "完整分类树"),
    ("GET", "/nodes/{id}", "分类/课程：路径、下级、资料"),
    ("GET", "/nodes/suggest?q=", "按名称/代号/拼音查课程"),
    ("POST", "/nodes", "新建分类 {parent, kind: section|group|course, name, code?, label?, aliases?, level?, sort?}"),
    ("PATCH", "/nodes/{id}", "审核员：改分类 {name?, code?, label?, aliases?, parent?, sort?, level?, bucketed?, approve?}"),
    ("POST", "/nodes/{id}/merge", "审核员：合并到另一分类 {into}"),
    ("DELETE", "/nodes/{id}", "审核员：删除空分类"),
    ("GET", "/search?q=&type=&tag=&within=&page=&level=&name_only=", "搜索"),
    ("GET", "/recent", "最新资料"),
    ("GET", "/popular", "热门资料"),
    ("GET", "/mine?q=&status=&offset=&limit=", "我上传的资料"),
    ("GET", "/resources/{id}", "资料详情"),
    ("POST", "/resources", "提交上传 {upload_id, node, type_word, time?, paper?, with_answer?, extra?, note?, subtitle?, major?}"),
    ("POST", "/resources/preview-name", "预览文件名 {…同上, ext}"),
    ("PATCH", "/resources/{id}", "改资料 {node, type_word, …同上}"),
    ("POST", "/resources/move", "审核员：批量移动 {ids, node}"),
    ("GET", "/resources/{id}/change-request", "我对这份资料的申请"),
    ("POST", "/resources/{id}/change-request", "上传者申请 {kind: note|delete, value}"),
    ("POST", "/resources/{id}/review", "审核员：{action: approve|reject|remove|restrict|restore, note?}"),
    ("POST", "/resources/{id}/report", "举报 {reason}"),
    ("GET", "/resources/{id}/download", "下载地址（每个分卷一组镜像 URL 和 sha256）"),
    ("GET", "/resources/{id}/social", "评分和评论"),
    ("PUT", "/resources/{id}/rating", "打分 {stars: 0–5}"),
    ("POST", "/resources/{id}/comments", "发评论 {body}"),
    ("DELETE", "/comments/{id}", "删评论（本人或审核员）"),
    ("GET", "/resources/{id}/reviews", "审核记录"),
    ("POST", "/resources/{id}/questions", "审核员向上传者提问 {text}"),
    ("POST", "/questions/{id}/answer", "上传者回答 {text}"),
    ("GET", "/me/questions", "审核员问我的问题"),
    ("POST", "/feedback", "意见反馈 {body, contact?, page?}"),
    ("GET", "/feedback/mine", "我的反馈和回复"),
    ("POST", "/uploads", "开始上传 {filename, mime?, parts: [{size, sha256}]}"),
    ("GET", "/uploads/{id}", "上传进度"),
    ("POST", "/uploads/{id}/parts/{index}", "确认分卷 {asset_id?}"),
    ("POST", "/uploads/{id}/parts/{index}/renew?via=relay", "换发送目标"),
    ("GET", "/wants?status=open|found|mine|pending&node=", "求资料帖"),
    ("POST", "/wants", "发帖 {title, body?, node?}"),
    ("GET", "/wants/{id}", "帖子和回复"),
    ("POST", "/wants/{id}/review", "审核员 {approve, note?}"),
    ("POST", "/wants/{id}/status", "{status: found|closed|open, resource?}"),
    ("PUT", "/wants/{id}/vote", "{on}"),
    ("POST", "/wants/{id}/replies", "回复 {body?, resource?}"),
    ("DELETE", "/want-replies/{id}", "删回复"),
    ("GET", "/stats", "公开统计：总数、下载最多/资料最多/近 30 天新增最多的课程、每周新增"),
    ("GET", "/notices?limit=", "我的站内提醒和未读数"),
    ("POST", "/notices/read", "标为已读 {id?}（不给 id 则全部）"),
    ("GET", "/nodes/{id}/follow", "我是否关注了这门课/学院"),
    ("PUT", "/nodes/{id}/follow", "关注或取消 {on}（有新资料审核通过时会收到提醒）"),
    ("GET", "/me/follows", "我关注的课程/学院"),
    ("GET", "/collections?scope=mine|public", "我的收藏夹 / 大家公开分享的收藏夹"),
    ("POST", "/collections", "新建收藏夹 {title, note?}"),
    ("GET", "/collections/{id}", "收藏夹和里面的资料"),
    ("PATCH", "/collections/{id}", "改名/改说明 {title, note?}（已分享的会重新审核）"),
    ("DELETE", "/collections/{id}", "删除收藏夹（资料本身不受影响）"),
    ("PUT", "/collections/{id}/items/{resource}", "放入或拿出一份资料 {on}"),
    ("POST", "/collections/{id}/share", "申请公开分享或取消分享 {on}（审核员通过后公开）"),
    ("GET", "/resources/{id}/collections", "这份资料在我的哪些收藏夹里"),
    ("GET", "/review/collections", "审核员：申请公开的收藏夹"),
    ("GET", "/series/{id}", "合集（一门课里按顺序排好的一组资料）；课程和资料详情里的 series 字段也有"),
    ("POST", "/series", "提议新合集 {node, title, source?（来源）, year?（年份）, items:[资料 id，按顺序]}（至少两份，同一门课，审核通过后显示；自己刚上传、还在审核的资料也可以放）"),
    ("PUT", "/series/{id}", "提议修改合集 {title, source?, year?, items}；items 为空表示解散（同样要审核）"),
    ("GET", "/review/series", "审核员：等待审核的合集提议"),
    ("POST", "/admin/users/{id}/purge", "管理员：封禁并清理账号 {reason?}：资料未通过/下架（文件不删）、评论和回复删除、评分撤销、求资料/合集提议/推荐链接驳回、收藏夹取消公开、头像和令牌清除"),
    ("POST", "/series/{id}/review", "审核员：{approve, note?（不通过时必填）, with_files?（通过时连同里面待审的资料一起通过）}"),
    ("POST", "/collections/{id}/review", "审核员：{approve, note?（不通过时必填）}"),
    ("GET", "/links", "友情链接"),
        ("PATCH", "/links/{id}", "审核员：改友情链接的名称、说明、排序 {title, url（不能改）, note?, sort?}"),
    ("DELETE", "/links/{id}", "审核员：删友情链接"),
    ("POST", "/links/suggestions", "推荐一个站外资源链接 {title, url, note?}（审核员通过后加入列表）"),
    ("GET", "/links/suggestions", "我推荐的链接（审核员：待审核的推荐）"),
    ("POST", "/links/suggestions/{id}/review", "审核员：{approve, reason?（不采纳时必填）, title?, url?, note?, sort?}"),
    ("GET", "/review?status=&uncertain=", "审核员：审核队列"),
    ("GET", "/review/batch?within=", "审核员：我领的这批"),
    ("POST", "/review/batch", "审核员：领一批 {within?}"),
    ("POST", "/review/batch/release", "审核员：放回这批"),
    ("POST", "/review/batch/skip/{id}", "审核员：跳过一份"),
    ("GET", "/review/nodes", "审核员：待确认的课程"),
    ("GET", "/review/change-requests", "审核员：上传者的申请"),
    ("POST", "/review/change-requests/{id}", "审核员：{approve, note?}"),
    ("PUT", "/announcement", "管理员：设公告 {text}（空字符串为不显示）"),
    ("GET", "/admin/avatars", "审核员：待审头像"),
    ("POST", "/admin/avatars/{user}", "审核员：{action: approve|reject}"),
    ("GET", "/admin/reports?all=", "审核员：举报"),
    ("POST", "/admin/reports/{id}/handle", "审核员：处理举报 {note?}"),
    ("DELETE", "/admin/reports/{id}", "审核员：删除举报"),
    ("GET", "/admin/feedback?all=", "管理员：意见反馈"),
    ("POST", "/admin/feedback/{id}/handle", "管理员：回复/处理反馈 {note?}"),
    ("GET", "/admin/reviews", "审核记录"),
    ("GET", "/admin/daily?days=", "每日统计"),
    ("GET", "/admin/users?q=", "管理员：用户"),
    ("PATCH", "/admin/users/{id}", "管理员：{level?, banned?}"),
    ("GET", "/admin/status", "管理员：运行状态、镜像探测"),
    ("POST", "/admin/github/scan", "管理员：扫描 GitHub 仓库 {url, depth?}"),
    ("POST", "/admin/github/import", "管理员：导入 {scan_id, depth, mappings, major?}"),
    ("GET", "/admin/github/transfer", "管理员：转存状态"),
    ("POST", "/admin/github/transfer", "管理员：开始转存"),
    ("GET", "/admin/thumbs", "管理员：缩略图状态"),
    ("POST", "/admin/thumbs", "管理员：生成缩略图"),
];

/// Paths `call_api` refuses: signing in / up and minting tokens stay on the website, and the
/// byte endpoints are the upload targets' business, not JSON calls.
const DENIED: &[&str] = &["/auth/", "/me/tokens", "/relay/", "/local/"];

/// What the tools need from the MCP request itself.
struct Ctx {
    /// `https://host` as the client reached us, for absolute links.
    base: String,
    /// Headers passed on to in-process `/api` calls (the token, the client address).
    forward: Vec<(header::HeaderName, HeaderValue)>,
}

impl Ctx {
    fn from(h: &HeaderMap, auth: &Auth) -> Ctx {
        let host = h.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("xmu.vintces.icu");
        let local = host.starts_with("localhost") || host.starts_with("127.0.0.1") || host.starts_with("[::1]");
        let scheme = h.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).unwrap_or(if local { "http" } else { "https" });
        let mut forward = Vec::new();
        // Only a token that resolved to a user is passed on (a cookie never is; see `post`).
        if auth.user.is_some()
            && let Some(v) = h.get(header::AUTHORIZATION)
        {
            forward.push((header::AUTHORIZATION, v.clone()));
        }
        if let Some(v) = h.get("x-real-ip") {
            forward.push((header::HeaderName::from_static("x-real-ip"), v.clone()));
        }
        Ctx { base: format!("{scheme}://{host}"), forward }
    }
}

fn encode(s: &str) -> String {
    s.as_bytes()
        .iter()
        .copied()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// `/api`-relative path plus query string, or why it's refused.
fn api_path(path: &str, query: Option<&Value>) -> Result<String, String> {
    let path = path.trim();
    let path = path.strip_prefix("/api").filter(|p| p.starts_with('/')).unwrap_or(path);
    let (p, q) = path.split_once('?').unwrap_or((path, ""));
    if !p.starts_with('/') || p.contains("..") || p.contains("//") || p.contains('#') || p.contains('\\') {
        return Err(format!("接口路径不对：{path}"));
    }
    let lower = p.to_ascii_lowercase();
    if DENIED.iter().any(|d| lower.starts_with(d) || lower == d.trim_end_matches('/')) {
        return Err("这个接口不能通过 MCP 调用（登录注册、令牌管理请在网站上操作）".into());
    }
    let mut qs: Vec<String> = q.split('&').filter(|x| !x.is_empty()).map(str::to_string).collect();
    if let Some(Value::Object(m)) = query {
        for (k, v) in m {
            let v = match v {
                Value::Null => continue,
                Value::String(s) => s.clone(),
                v => v.to_string(),
            };
            qs.push(format!("{}={}", encode(k), encode(&v)));
        }
    }
    Ok(if qs.is_empty() { format!("/api{p}") } else { format!("/api{p}?{}", qs.join("&")) })
}

/// Upload targets on this site (the relay, local storage) are relative and, being unsafe
/// same-origin requests, need the anti-CSRF header the browser adds; spell both out.
fn absolute_targets(v: &mut Value, base: &str) {
    match v {
        Value::Object(m) => {
            let relative = m.get("method").is_some() && m.get("url").and_then(Value::as_str).is_some_and(|u| u.starts_with("/api/"));
            if relative {
                let url = format!("{base}{}", m["url"].as_str().unwrap_or_default());
                m.insert("url".into(), json!(url));
                if let Some(Value::Array(h)) = m.get_mut("headers") {
                    h.push(json!([CSRF_HEADER, "1"]));
                }
            }
            m.values_mut().for_each(|x| absolute_targets(x, base));
        }
        Value::Array(a) => a.iter_mut().for_each(|x| absolute_targets(x, base)),
        _ => {}
    }
}

/// Largest JSON answer read back from an in-process call (the whole tree is a few MB).
const MAX_ANSWER: usize = 8 << 20;

/// Runs one `/api` request through the site's own router, as the token's owner.
async fn call_api(app: &Arc<App>, ctx: &Ctx, method: &str, path: &str, query: Option<&Value>, body: Option<&Value>) -> Result<Value, String> {
    let method = Method::from_bytes(method.to_ascii_uppercase().as_bytes()).map_err(|_| format!("不支持的方法 {method}"))?;
    let uri = api_path(path, query)?;
    let mut req = Request::builder().method(method).uri(&uri);
    for (k, v) in &ctx.forward {
        req = req.header(k, v);
    }
    // No cookie is ever passed on, so this can't be a forged browser request.
    req = req.header(CSRF_HEADER, "1");
    let req = match body.filter(|b| !b.is_null()) {
        Some(b) => req.header(header::CONTENT_TYPE, "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .map_err(|e| e.to_string())?;
    let res = crate::api::router(app.clone()).oneshot(req).await.map_err(|e| e.to_string())?;
    let status = res.status();
    let raw = axum::body::to_bytes(res.into_body(), MAX_ANSWER).await.map_err(|e| e.to_string())?;
    let mut v: Value = serde_json::from_slice(&raw).unwrap_or_else(|_| json!(String::from_utf8_lossy(&raw)));
    if !status.is_success() {
        let msg = v.get("error").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| v.to_string());
        return Err(format!("HTTP {}：{msg}", status.as_u16()));
    }
    absolute_targets(&mut v, &ctx.base);
    Ok(v)
}

fn arg_id(a: &Value, k: &str) -> Result<Id, String> {
    a.get(k).and_then(Value::as_u64).ok_or_else(|| format!("缺少参数 {k}"))
}

fn arg_str<'a>(a: &'a Value, k: &str) -> &'a str {
    a.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Runs a (possibly fsync-ing) hub call off the async executor.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> xmuhub_core::Result<T> + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}

async fn call_tool(app: &Arc<App>, auth: &Auth, ctx: &Ctx, ip: &str, name: &str, a: &Value) -> Result<Value, String> {
    let v = auth.viewer();
    let brief = |r: &xmuhub_core::model::Resource, n: &xmuhub_core::model::Node| {
        let mut x = resource_view(app, r, n, &[], v);
        if let Some(o) = x.as_object_mut() {
            o.insert("url".into(), json!(format!("{}/r/{}", ctx.base, r.id)));
        }
        x
    };
    Ok(match name {
        "search" => {
            let filter = Filter {
                ty: match arg_str(a, "type") {
                    "node" => Some(DocType::Node),
                    "resource" => Some(DocType::Resource),
                    _ => None,
                },
                courses_only: false,
                level: None,
                name_only: false,
                within: a.get("within").and_then(Value::as_u64),
                tag: Tag::parse(arg_str(a, "tag")).and_then(|t| Tag::ALL.iter().position(|x| *x == t)).map(|i| i as u64),
            };
            let page = a.get("page").and_then(Value::as_u64).unwrap_or(1).clamp(1, 50) as usize;
            let (items, total) = app.hub.search(v, arg_str(a, "query"), filter, 20, (page - 1) * 20).map_err(|e| e.to_string())?;
            let items: Vec<Value> = items
                .iter()
                .map(|i| match i {
                    SearchItem::Node { node, path, count } => json!({
                        "type": "node", "id": node.id, "name": node.name, "kind": node.kind.as_str(), "count": count,
                        "path": path.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(" / "),
                    }),
                    SearchItem::Resource { resource, node, .. } => {
                        let mut x = brief(resource, node);
                        x["type"] = json!("resource");
                        x
                    }
                })
                .collect();
            json!({ "total": total, "page": page, "items": items })
        }
        "get_tree" => {
            let parent = a.get("parent").and_then(Value::as_u64);
            let list: Vec<Value> =
                app.hub.tree().iter().filter(|i| i.node.parent == parent).map(|i| node_view(&app.hub, &i.node, i.count)).collect();
            json!(list)
        }
        "get_category" => {
            let (info, path, children, resources) = app.hub.node(v, arg_id(a, "id")?).map_err(|e| e.to_string())?;
            json!({
                "node": node_view(&app.hub, &info.node, info.count),
                "path": path.iter().map(|p| json!({ "id": p.id, "name": p.name })).collect::<Vec<_>>(),
                "children": children.iter().map(|c| node_view(&app.hub, &c.node, c.count)).collect::<Vec<_>>(),
                "resources": resources.iter().map(|r| brief(r, &info.node)).collect::<Vec<_>>(),
            })
        }
        "get_resource" => {
            let (r, n, path) = app.hub.resource(v, arg_id(a, "id")?).map_err(|e| e.to_string())?;
            let mut x = resource_view(app, &r, &n, &path, v);
            x["url"] = json!(format!("{}/r/{}", ctx.base, r.id));
            x
        }
        "get_download_links" => json!(app.hub.download(v, arg_id(a, "id")?, Some(ip)).map_err(|e| e.to_string())?),
        "list_recent" | "list_popular" => {
            let limit = a.get("limit").and_then(Value::as_u64).unwrap_or(12).clamp(1, 50) as usize;
            let list = if name == "list_recent" { app.hub.recent(limit) } else { app.hub.popular(limit) };
            json!(list.iter().map(|(r, n)| brief(r, n)).collect::<Vec<_>>())
        }
        "get_comments" => {
            let id = arg_id(a, "id")?;
            let comments = app.hub.comments(v, id).map_err(|e| e.to_string())?;
            json!({ "rating": app.hub.rating(id), "comments": comments })
        }
        "post_comment" => {
            let (hub, user, id, body) = (app.hub.clone(), auth.user.clone(), arg_id(a, "id")?, arg_str(a, "body").to_string());
            let c = blocking(move || hub.add_comment(Viewer { user: user.as_ref() }, id, &body)).await?;
            json!({ "ok": true, "comment_id": c.id })
        }
        "rate_resource" => {
            let stars = a.get("stars").and_then(Value::as_u64).ok_or("缺少参数 stars")?.min(255) as u8;
            let (hub, user, id) = (app.hub.clone(), auth.user.clone(), arg_id(a, "id")?);
            json!({ "rating": blocking(move || hub.rate(Viewer { user: user.as_ref() }, id, stars)).await? })
        }
        "whoami" => match &auth.user {
            Some(u) => {
                let role = ["访客", "贡献者", "可信贡献者", "审核员", "管理员"][u.level as usize];
                json!({ "nickname": u.nickname, "level": u.level as u8, "role": role })
            }
            None => json!({ "role": "访客", "hint": "在网站「我的」页面创建个人令牌，以 Authorization: Bearer <令牌> 连接即可使用写操作" }),
        },
        "submit_feedback" => {
            let (hub, user, ip) = (app.hub.clone(), auth.user.clone(), ip.to_string());
            let (body, contact) = (arg_str(a, "body").to_string(), arg_str(a, "contact").to_string());
            blocking(move || hub.submit_feedback(Viewer { user: user.as_ref() }, &body, &contact, "mcp", &ip)).await?;
            json!({ "ok": true })
        }
        "review_queue" => {
            let status = Some(arg_str(a, "status")).filter(|s| !s.is_empty());
            let uncertain = a.get("uncertain_only").and_then(Value::as_bool).unwrap_or(false);
            let items = app.hub.review_queue(v, status, uncertain).map_err(|e| e.to_string())?;
            json!(items.iter().map(|(r, n)| brief(r, n)).collect::<Vec<_>>())
        }
        "review_resource" => {
            let (hub, user, id) = (app.hub.clone(), auth.user.clone(), arg_id(a, "id")?);
            let (action, note) = (arg_str(a, "action").to_string(), arg_str(a, "note").to_string());
            let r = blocking(move || hub.review(Viewer { user: user.as_ref() }, id, &action, &note)).await?;
            json!({ "ok": true, "status": r.status.as_str() })
        }
        "site_info" => call_api(app, ctx, "GET", "/meta", None, None).await?,
        "suggest_category" => call_api(app, ctx, "GET", "/nodes/suggest", Some(&json!({ "q": arg_str(a, "q") })), None).await?,
        "create_category" => {
            let kind = Some(arg_str(a, "kind")).filter(|k| !k.is_empty()).unwrap_or("course");
            let body = json!({ "parent": arg_id(a, "parent")?, "kind": kind, "name": arg_str(a, "name"), "level": a.get("level").and_then(Value::as_u64).unwrap_or(0) });
            call_api(app, ctx, "POST", "/nodes", None, Some(&body)).await?
        }
        "begin_upload" => {
            let body = json!({ "filename": arg_str(a, "filename"), "mime": arg_str(a, "mime"), "parts": a.get("parts").cloned().unwrap_or(Value::Null) });
            call_api(app, ctx, "POST", "/uploads", None, Some(&body)).await?
        }
        "get_upload" => call_api(app, ctx, "GET", &format!("/uploads/{}", arg_id(a, "upload_id")?), None, None).await?,
        "confirm_upload_part" => {
            let path = format!("/uploads/{}/parts/{}", arg_id(a, "upload_id")?, arg_id(a, "index")?);
            call_api(app, ctx, "POST", &path, None, Some(&json!({ "asset_id": a.get("asset_id") }))).await?
        }
        "renew_upload_part" => {
            let relay = a.get("via_relay").and_then(Value::as_bool).unwrap_or(false);
            let path = format!("/uploads/{}/parts/{}/renew{}", arg_id(a, "upload_id")?, arg_id(a, "index")?, if relay { "?via=relay" } else { "" });
            call_api(app, ctx, "POST", &path, None, None).await?
        }
        "preview_name" => {
            let mut body = a.get("fields").cloned().filter(Value::is_object).ok_or("缺少参数 fields")?;
            body["ext"] = json!(arg_str(a, "ext"));
            body["node"] = body.get("node").cloned().filter(|n| !n.is_null()).unwrap_or(json!(0));
            call_api(app, ctx, "POST", "/resources/preview-name", None, Some(&body)).await?
        }
        "publish_upload" => {
            let mut body = a.get("fields").cloned().filter(Value::is_object).ok_or("缺少参数 fields")?;
            // No course given: 「待整理」, like the upload page.
            if body.get("node").is_none_or(Value::is_null) {
                body["node"] = call_api(app, ctx, "POST", "/inbox", None, None).await?["id"].clone();
            }
            body["upload_id"] = json!(arg_id(a, "upload_id")?);
            call_api(app, ctx, "POST", "/resources", None, Some(&body)).await?
        }
        "my_uploads" => {
            let q = json!({ "q": a.get("q"), "status": a.get("status"), "offset": a.get("offset"), "limit": a.get("limit") });
            call_api(app, ctx, "GET", "/mine", Some(&q), None).await?
        }
        "edit_resource" => {
            let body = a.get("fields").cloned().filter(Value::is_object).ok_or("缺少参数 fields")?;
            call_api(app, ctx, "PATCH", &format!("/resources/{}", arg_id(a, "id")?), None, Some(&body)).await?
        }
        "request_resource_change" => {
            let body = json!({ "kind": arg_str(a, "kind"), "value": arg_str(a, "value") });
            call_api(app, ctx, "POST", &format!("/resources/{}/change-request", arg_id(a, "id")?), None, Some(&body)).await?
        }
        "report_resource" => {
            call_api(app, ctx, "POST", &format!("/resources/{}/report", arg_id(a, "id")?), None, Some(&json!({ "reason": arg_str(a, "reason") }))).await?
        }
        "move_resources" => {
            let body = json!({ "ids": a.get("ids").cloned().unwrap_or(json!([])), "node": arg_id(a, "node")? });
            call_api(app, ctx, "POST", "/resources/move", None, Some(&body)).await?
        }
        "list_wants" => {
            let q = json!({ "status": Some(arg_str(a, "status")).filter(|s| !s.is_empty()).unwrap_or("open"), "node": a.get("node") });
            call_api(app, ctx, "GET", "/wants", Some(&q), None).await?
        }
        "get_want" => call_api(app, ctx, "GET", &format!("/wants/{}", arg_id(a, "id")?), None, None).await?,
        "post_want" => {
            let body = json!({ "title": arg_str(a, "title"), "body": arg_str(a, "body"), "node": a.get("node") });
            call_api(app, ctx, "POST", "/wants", None, Some(&body)).await?
        }
        "reply_want" => {
            let body = json!({ "body": arg_str(a, "body"), "resource": a.get("resource") });
            call_api(app, ctx, "POST", &format!("/wants/{}/replies", arg_id(a, "id")?), None, Some(&body)).await?
        }
        "vote_want" => {
            let on = a.get("on").and_then(Value::as_bool).ok_or("缺少参数 on")?;
            call_api(app, ctx, "PUT", &format!("/wants/{}/vote", arg_id(a, "id")?), None, Some(&json!({ "on": on }))).await?
        }
        "set_want_status" => {
            let body = json!({ "status": arg_str(a, "status"), "resource": a.get("resource") });
            call_api(app, ctx, "POST", &format!("/wants/{}/status", arg_id(a, "id")?), None, Some(&body)).await?
        }
        "review_want" => {
            let approve = a.get("approve").and_then(Value::as_bool).ok_or("缺少参数 approve")?;
            let body = json!({ "approve": approve, "note": arg_str(a, "note") });
            call_api(app, ctx, "POST", &format!("/wants/{}/review", arg_id(a, "id")?), None, Some(&body)).await?
        }
        "site_stats" => call_api(app, ctx, "GET", "/stats", None, None).await?,
        "follow_course" => {
            let on = a.get("on").and_then(Value::as_bool).ok_or("缺少参数 on")?;
            call_api(app, ctx, "PUT", &format!("/nodes/{}/follow", arg_id(a, "id")?), None, Some(&json!({ "on": on }))).await?
        }
        "list_notices" => {
            let list = call_api(app, ctx, "GET", "/notices", Some(&json!({ "limit": a.get("limit") })), None).await?;
            if a.get("mark_read").and_then(Value::as_bool).unwrap_or(false) {
                call_api(app, ctx, "POST", "/notices/read", None, Some(&json!({}))).await?;
            }
            list
        }
        "favorite" => {
            let resource = arg_id(a, "resource")?;
            let on = a.get("on").and_then(Value::as_bool).unwrap_or(true);
            let id = match a.get("collection").and_then(Value::as_u64) {
                Some(id) => id,
                None => {
                    let mine = call_api(app, ctx, "GET", "/collections", None, None).await?;
                    let found = mine.as_array().and_then(|l| l.iter().find(|c| c["title"] == "我的收藏")).and_then(|c| c["id"].as_u64());
                    match found {
                        Some(id) => id,
                        None => call_api(app, ctx, "POST", "/collections", None, Some(&json!({ "title": "我的收藏" }))).await?["id"].as_u64().ok_or("新建收藏夹失败")?,
                    }
                }
            };
            let r = call_api(app, ctx, "PUT", &format!("/collections/{id}/items/{resource}"), None, Some(&json!({ "on": on }))).await?;
            json!({ "collection": id, "in": r["in"] })
        }
        "list_api" => json!({
            "note": "路径相对于 /api；{id} 等换成实际值；{…} 是 JSON 请求体。权限和网页上相同。",
            "routes": API.iter().map(|(m, p, d)| json!({ "method": m, "path": p, "use": d })).collect::<Vec<_>>(),
        }),
        "call_api" => call_api(app, ctx, arg_str(a, "method"), arg_str(a, "path"), a.get("query"), a.get("body")).await?,
        _ => return Err(format!("未知工具 {name}")),
    })
}

async fn handle_one(app: &Arc<App>, auth: &Auth, ctx: &Ctx, ip: &str, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    // Notifications (no id) and client responses get no reply.
    let id = id?;
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result: Result<Value, (i64, String)> = match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
            let version = if SUPPORTED.contains(&asked) { asked } else { SUPPORTED[0] };
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "ludao-shuge", "title": "鹭岛书阁 · 厦大资料库", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools(app.hub.limits.max_part, app.hub.limits.max_file) })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            Ok(match call_tool(app, auth, ctx, ip, name, &args).await {
                Ok(v) => json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() }], "isError": false }),
                Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
            })
        }
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
}

/// Calls in one batch request (each can be a search or a lookup).
const MAX_BATCH: usize = 10;

pub async fn post(State(app): State<Arc<App>>, auth: Auth, h: HeaderMap, body: axum::body::Bytes) -> Response {
    // Only a personal token identifies the caller here. A browser cookie is ignored: this
    // endpoint takes plain JSON without the anti-CSRF header, so honouring cookies would let
    // another page (a sibling subdomain, say) act as a signed-in reviewer.
    let auth = if auth.session.is_some() { Auth { user: None, session: None } } else { auth };
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "parse error" } }))).into_response();
    };
    let ip = client_ip(&h);
    let ctx = Ctx::from(&h, &auth);
    let reply = match &msg {
        Value::Array(batch) => {
            let mut out = Vec::new();
            for m in batch.iter().take(MAX_BATCH) {
                if let Some(r) = handle_one(&app, &auth, &ctx, &ip, m).await {
                    out.push(r);
                }
            }
            if out.is_empty() { None } else { Some(Value::Array(out)) }
        }
        m => handle_one(&app, &auth, &ctx, &ip, m).await,
    };
    match reply {
        Some(r) => Json(r).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// No server-initiated stream: tell clients to use plain POSTs.
pub async fn get() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(axum::http::header::ALLOW, "POST")]).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_paths_are_checked() {
        assert_eq!(api_path("/resources/1", None).unwrap(), "/api/resources/1");
        assert_eq!(api_path("/api/wants?status=open", Some(&json!({ "node": 7, "q": "数据 结构" }))).unwrap(), "/api/wants?status=open&node=7&q=%E6%95%B0%E6%8D%AE%20%E7%BB%93%E6%9E%84");
        for bad in ["/auth/login", "/me/tokens", "/me/tokens/3", "/api/relay/upload?t=x", "/local/file/a", "/../mcp", "resources", "//x"] {
            assert!(api_path(bad, None).is_err(), "{bad}");
        }
        assert!(api_path("/me", None).is_ok());
    }

    #[test]
    fn relative_upload_targets_become_absolute() {
        let mut v = json!({ "parts": [
            { "target": { "url": "/api/relay/upload?t=1", "method": "POST", "headers": [] } },
            { "target": { "url": "https://upload.example/upload?t=2", "method": "POST", "headers": [] } },
        ] });
        absolute_targets(&mut v, "https://xmu.example");
        assert_eq!(v["parts"][0]["target"]["url"], "https://xmu.example/api/relay/upload?t=1");
        assert_eq!(v["parts"][0]["target"]["headers"], json!([[CSRF_HEADER, "1"]]));
        assert_eq!(v["parts"][1]["target"]["headers"], json!([]));
    }
}
