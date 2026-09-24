//! Model Context Protocol endpoint (`POST /mcp`, Streamable HTTP transport, JSON responses).
//!
//! Lets AI agents search and browse the catalogue, fetch download links, and — with a
//! personal access token (`Authorization: Bearer xmh_…`, created on the 「我的」 page) —
//! comment, rate, and for reviewers work the review queue. The token acts as its owner.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use xmuhub_core::hub::{SearchItem, Viewer};
use xmuhub_core::model::{Id, Tag};
use xmuhub_core::search::{DocType, Filter};

use crate::api::{App, Auth, client_ip, node_view, resource_view};

const SUPPORTED: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "XMUHub 是厦门大学学生资料共享站（往年试卷、笔记、课件、题库）。\
先用 search 找课程或资料（支持中文、课程代号和拼音首字母，如 wjf=微积分），\
用 get_category 浏览课程下的资料，用 get_download_links 取下载地址（国内镜像，按速度排序，依次尝试即可）。\
资料 id 和分类 id 都是数字。写操作（评论、评分、审核）需要个人令牌。";

fn tools() -> Value {
    let id = |d: &str| json!({ "type": "integer", "description": d });
    json!([
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
    ])
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

async fn call_tool(app: &Arc<App>, auth: &Auth, ip: &str, name: &str, a: &Value) -> Result<Value, String> {
    let v = auth.viewer();
    let brief = |r: &xmuhub_core::model::Resource, n: &xmuhub_core::model::Node| {
        let mut x = resource_view(app, r, n, &[], v);
        if let Some(o) = x.as_object_mut() {
            o.insert("url".into(), json!(format!("https://xmu.vintces.icu/r/{}", r.id)));
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
                app.hub.tree().iter().filter(|i| i.node.parent == parent).map(|i| node_view(&i.node, i.count)).collect();
            json!(list)
        }
        "get_category" => {
            let (info, path, children, resources) = app.hub.node(v, arg_id(a, "id")?).map_err(|e| e.to_string())?;
            json!({
                "node": node_view(&info.node, info.count),
                "path": path.iter().map(|p| json!({ "id": p.id, "name": p.name })).collect::<Vec<_>>(),
                "children": children.iter().map(|c| node_view(&c.node, c.count)).collect::<Vec<_>>(),
                "resources": resources.iter().map(|r| brief(r, &info.node)).collect::<Vec<_>>(),
            })
        }
        "get_resource" => {
            let (r, n, path) = app.hub.resource(v, arg_id(a, "id")?).map_err(|e| e.to_string())?;
            let mut x = resource_view(app, &r, &n, &path, v);
            x["url"] = json!(format!("https://xmu.vintces.icu/r/{}", r.id));
            x
        }
        "get_download_links" => json!(app.hub.download(v, arg_id(a, "id")?, true).map_err(|e| e.to_string())?),
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
            let (r, garbage) = blocking(move || hub.review(Viewer { user: user.as_ref() }, id, &action, &note)).await?;
            crate::api::delete_later(app, garbage);
            json!({ "ok": true, "status": r.status.as_str() })
        }
        _ => return Err(format!("未知工具 {name}")),
    })
}

async fn handle_one(app: &Arc<App>, auth: &Auth, ip: &str, msg: &Value) -> Option<Value> {
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
                "serverInfo": { "name": "xmuhub", "title": "XMUHub 厦大资料库", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            Ok(match call_tool(app, auth, ip, name, &args).await {
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

pub async fn post(State(app): State<Arc<App>>, auth: Auth, h: HeaderMap, body: axum::body::Bytes) -> Response {
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "parse error" } }))).into_response();
    };
    let ip = client_ip(&h);
    let reply = match &msg {
        Value::Array(batch) => {
            let mut out = Vec::new();
            for m in batch.iter().take(50) {
                if let Some(r) = handle_one(&app, &auth, &ip, m).await {
                    out.push(r);
                }
            }
            if out.is_empty() { None } else { Some(Value::Array(out)) }
        }
        m => handle_one(&app, &auth, &ip, m).await,
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
