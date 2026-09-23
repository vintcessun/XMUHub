//! The category tree (目录即分类): sections → groups → courses → levels.

use serde::{Deserialize, Serialize};

use super::{Hub, Viewer, clean};
use crate::error::{Error, Result, bad};
use crate::model::*;

#[derive(Debug, Clone, Deserialize)]
pub struct NodeInput {
    pub parent: Option<Id>,
    pub kind: String,
    #[serde(default)]
    pub code: String,
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub bucketed: bool,
    #[serde(default)]
    pub sort: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NodePatch {
    pub code: Option<String>,
    pub name: Option<String>,
    pub label: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub bucketed: Option<bool>,
    pub sort: Option<u32>,
    pub parent: Option<Id>,
    #[serde(default)]
    pub approve: bool,
}

/// A node plus what pages need to render it.
#[derive(Debug, Clone, Serialize)]
pub struct NodeInfo {
    pub node: Node,
    pub count: usize,
}

fn segment(s: &str, max: usize) -> String {
    // Name segments feed file names: no separators or brackets that would break 课程_时间_类型.
    clean(s, max).chars().filter(|c| !"_/\\()（）【】[] ".contains(*c)).collect()
}

impl Hub {
    fn info(st: &super::State, n: &Node) -> NodeInfo {
        NodeInfo { node: n.clone(), count: st.counts.get(&n.id).copied().unwrap_or(0) }
    }

    /// The whole live tree, flat (clients assemble it by `parent`).
    pub fn tree(&self) -> Vec<NodeInfo> {
        let st = self.st.read();
        let mut out = Vec::with_capacity(st.nodes.len());
        let mut stack: Vec<Id> = st.children.get(&0).cloned().unwrap_or_default();
        stack.reverse();
        while let Some(id) = stack.pop() {
            if let Some(n) = st.nodes.get(&id) {
                out.push(Self::info(&st, n));
            }
            if let Some(c) = st.children.get(&id) {
                stack.extend(c.iter().rev());
            }
        }
        out
    }

    /// A node with its ancestors, children and directly attached resources.
    pub fn node(&self, viewer: Viewer, id: Id) -> Result<(NodeInfo, Vec<Node>, Vec<NodeInfo>, Vec<Resource>)> {
        let st = self.st.read();
        let n = st.resolve(id).ok_or(Error::NotFound("分类"))?;
        let path = st.ancestors(n.id).iter().filter_map(|a| st.nodes.get(a).cloned()).collect();
        let children = st.children.get(&n.id).into_iter().flatten().filter_map(|c| st.nodes.get(c)).map(|c| Self::info(&st, c)).collect();
        let mut resources: Vec<Resource> = st
            .by_node
            .get(&n.id)
            .into_iter()
            .flatten()
            .map(|r| &st.resources[r])
            .filter(|r| viewer.can_see(r))
            .cloned()
            .collect();
        resources.sort_by(|a, b| b.name.time.cmp(&a.name.time).then_with(|| a.name.stem().cmp(&b.name.stem())));
        Ok((Self::info(&st, n), path, children, resources))
    }

    /// Nodes an uploader may pick, matched by name/label/alias/code/pinyin.
    pub fn suggest_nodes(&self, q: &str, limit: usize) -> Vec<(Node, Vec<Node>)> {
        let st = self.st.read();
        let q = q.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(u8, &Node)> = st
            .nodes
            .values()
            .filter(|n| !matches!(n.status, NodeStatus::Merged(_)) && n.kind != NodeKind::Section)
            .filter_map(|n| {
                let name = n.name.to_lowercase();
                let label = n.label.to_lowercase();
                let code = n.code.to_lowercase();
                let py = crate::text::pinyin_forms(&format!("{} {}", n.name, n.aliases.join(" ")));
                let score = if name == q || label == q || code == q {
                    0
                } else if name.starts_with(&q) || label.starts_with(&q) || code.starts_with(&q) {
                    1
                } else if name.contains(&q) || label.contains(&q) || n.aliases.iter().any(|a| a.to_lowercase().contains(&q)) {
                    2
                } else if py.split(' ').any(|p| !p.is_empty() && p.starts_with(&q)) {
                    3
                } else {
                    return None;
                };
                // Prefer the leaves people actually upload into.
                let kind_bias = match n.kind {
                    NodeKind::Level | NodeKind::Course => 0,
                    _ => 1,
                };
                Some((score * 2 + kind_bias, n))
            })
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.code.cmp(&b.1.code)).then_with(|| a.1.name.len().cmp(&b.1.name.len())));
        scored
            .into_iter()
            .take(limit)
            .map(|(_, n)| (n.clone(), st.ancestors(n.id).iter().filter_map(|a| st.nodes.get(a).cloned()).collect()))
            .collect()
    }

    fn default_label(st: &super::State, parent: Option<Id>, kind: NodeKind, name: &str) -> String {
        let parent_label = parent.and_then(|p| st.nodes.get(&p)).map(|p| p.label.as_str()).unwrap_or("");
        if kind != NodeKind::Level || parent_label.is_empty() {
            return segment(name, 40);
        }
        // Levels extend the course name: 微积分I + I-1 → 微积分I-1, 大学物理A + 上 → 大学物理A-上.
        let roman: String = name.chars().take_while(|c| matches!(c, 'I' | 'V' | 'X')).collect();
        if !roman.is_empty() && parent_label.ends_with(&roman) {
            return segment(&format!("{}{}", &parent_label[..parent_label.len() - roman.len()], name), 40);
        }
        if name.chars().all(|c| "上下".contains(c)) {
            return segment(&format!("{parent_label}-{name}"), 40);
        }
        segment(name, 40)
    }

    pub fn create_node(&self, actor: Viewer, input: NodeInput) -> Result<Node> {
        let me = actor.at_least(Level::Contributor)?.clone();
        let kind = NodeKind::parse(&input.kind).ok_or_else(|| bad("未知的节点类型"))?;
        let name = clean(&input.name, 60);
        if name.chars().count() < 1 {
            return Err(bad("名称不能为空"));
        }
        let staff = me.level >= Level::Reviewer;
        let n = self.mutate(|st, tx| {
            let parent = match input.parent {
                Some(p) => Some(st.resolve(p).map(|n| n.id).ok_or(Error::NotFound("上级分类"))?),
                None => None,
            };
            if !staff {
                // Uploaders may only add a missing course under an existing group (e.g. a college).
                let pk = parent.and_then(|p| st.nodes.get(&p)).map(|p| p.kind);
                if kind != NodeKind::Course || pk != Some(NodeKind::Group) {
                    return Err(Error::Forbidden);
                }
            }
            if parent.is_none() && kind != NodeKind::Section {
                return Err(bad("只有一级栏目可以没有上级"));
            }
            // Reuse an existing sibling with the same name instead of duplicating it.
            if let Some(existing) = st.children.get(&parent.unwrap_or(0)).into_iter().flatten().filter_map(|c| st.nodes.get(c)).find(|c| c.name == name) {
                return Ok(existing.clone());
            }
            let label = if input.label.trim().is_empty() { Self::default_label(st, parent, kind, &name) } else { segment(&input.label, 40) };
            let n = Node {
                id: st.next_id(tx)?,
                parent,
                kind,
                code: clean(&input.code, 20),
                name,
                label,
                aliases: input.aliases.iter().map(|a| clean(a, 30)).filter(|a| !a.is_empty()).take(20).collect(),
                bucketed: input.bucketed,
                sort: input.sort,
                status: if staff { NodeStatus::Active } else { NodeStatus::Pending },
                created_by: me.id,
                created_at: now(),
            };
            st.put_node(tx, n.clone())?;
            Ok(n)
        })?;
        self.reindex(&[n.id], &[]);
        Ok(n)
    }

    pub fn update_node(&self, actor: Viewer, id: Id, p: NodePatch) -> Result<Node> {
        actor.at_least(Level::Reviewer)?;
        let (n, touched) = self.mutate(|st, tx| {
            let mut n = st.nodes.get(&id).cloned().ok_or(Error::NotFound("分类"))?;
            if let Some(v) = p.code {
                n.code = clean(&v, 20);
            }
            if let Some(v) = p.name {
                let v = clean(&v, 60);
                if v.is_empty() {
                    return Err(bad("名称不能为空"));
                }
                n.name = v;
            }
            if let Some(v) = p.label {
                n.label = segment(&v, 40);
            }
            if let Some(v) = p.aliases {
                n.aliases = v.iter().map(|a| clean(a, 30)).filter(|a| !a.is_empty()).take(20).collect();
            }
            if let Some(v) = p.bucketed {
                n.bucketed = v;
            }
            if let Some(v) = p.sort {
                n.sort = v;
            }
            if let Some(pid) = p.parent {
                let target = st.resolve(pid).map(|t| t.id).ok_or(Error::NotFound("上级分类"))?;
                if target == n.id || st.ancestors(target).contains(&n.id) {
                    return Err(bad("不能移动到自己的下级"));
                }
                n.parent = Some(target);
            }
            if p.approve && n.status == NodeStatus::Pending {
                n.status = NodeStatus::Active;
            }
            st.put_node(tx, n.clone())?;
            // Renames change every descendant's search text.
            let touched = st.subtree(n.id);
            Ok((n, touched))
        })?;
        let resources: Vec<Id> = {
            let st = self.st.read();
            touched.iter().flat_map(|t| st.by_node.get(t).cloned().unwrap_or_default()).collect()
        };
        self.reindex(&touched, &resources);
        Ok(n)
    }

    /// Moves every resource and child of `from` into `into` and retires `from`.
    pub fn merge_node(&self, actor: Viewer, from: Id, into: Id) -> Result<Node> {
        actor.at_least(Level::Reviewer)?;
        if from == into {
            return Err(bad("不能合并到自己"));
        }
        let (target, moved, kids) = self.mutate(|st, tx| {
            let mut src = st.nodes.get(&from).cloned().ok_or(Error::NotFound("分类"))?;
            let mut dst = st.resolve(into).cloned().ok_or(Error::NotFound("目标分类"))?;
            if matches!(src.status, NodeStatus::Merged(_)) || st.ancestors(dst.id).contains(&src.id) {
                return Err(bad("无法合并"));
            }
            let moved: Vec<Id> = st.by_node.get(&from).cloned().unwrap_or_default();
            for rid in &moved {
                let mut r = st.resources[rid].clone();
                r.node = dst.id;
                r.updated_at = now();
                st.put_resource(tx, r)?;
            }
            let kids: Vec<Id> = st.children.get(&from).cloned().unwrap_or_default();
            for k in &kids {
                let mut c = st.nodes[k].clone();
                c.parent = Some(dst.id);
                st.put_node(tx, c)?;
            }
            for a in std::iter::once(src.name.clone()).chain(src.aliases.iter().cloned()) {
                if a != dst.name && !dst.aliases.contains(&a) {
                    dst.aliases.push(a);
                }
            }
            src.status = NodeStatus::Merged(dst.id);
            st.put_node(tx, src)?;
            st.put_node(tx, dst.clone())?;
            Ok((dst, moved, kids))
        })?;
        let mut nodes = vec![from, target.id];
        nodes.extend(kids);
        self.reindex(&nodes, &moved);
        Ok(target)
    }

    /// Deletes an empty node (no children, no resources).
    pub fn delete_node(&self, actor: Viewer, id: Id) -> Result<()> {
        actor.at_least(Level::Reviewer)?;
        self.mutate(|st, tx| {
            if st.children.get(&id).is_some_and(|c| !c.is_empty()) || st.by_node.get(&id).is_some_and(|r| !r.is_empty()) {
                return Err(Error::Conflict("只能删除空的分类".into()));
            }
            st.nodes.remove(&id).ok_or(Error::NotFound("分类"))?;
            tx.del_node(id)
        })?;
        self.reindex(&[id], &[]);
        Ok(())
    }

    pub fn pending_nodes(&self, actor: Viewer) -> Result<Vec<(Node, Vec<Node>, usize)>> {
        actor.at_least(Level::Reviewer)?;
        let st = self.st.read();
        let mut v: Vec<(Node, Vec<Node>, usize)> = st
            .nodes
            .values()
            .filter(|n| n.status == NodeStatus::Pending)
            .map(|n| (n.clone(), st.ancestors(n.id).iter().filter_map(|a| st.nodes.get(a).cloned()).collect(), st.by_node.get(&n.id).map(Vec::len).unwrap_or(0)))
            .collect();
        v.sort_by_key(|(n, _, _)| n.created_at);
        Ok(v)
    }
}
