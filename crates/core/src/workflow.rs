//! Versioned workflow authoring contract. Runtime state is deliberately separate.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const MAX_NODES: usize = 128;
const MAX_FLOWS: usize = 512;
const MAX_NAME_BYTES: usize = 512;
const MAX_TEXT_BYTES: usize = 128_000;
const MAX_POSITION: f64 = 100_000.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Group {
    pub id: String,
    pub flow_ids: Vec<String>,
    pub min: usize,
    pub max: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rule {
    pub mode: Mode,
    pub min: usize,
    pub max: usize,
    pub required: Vec<String>,
    pub groups: Vec<Group>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    All,
    One,
    Any,
    Range,
    Custom,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub template_id: Option<String>,
    /// Blank nodes select a configured model; template nodes use their template's model.
    /// Existing v1 graphs omitted this field and remain editable drafts.
    #[serde(default)]
    pub model_config_id: Option<String>,
    pub receives: String,
    pub task: String,
    pub delivers: String,
    pub input_rule: Rule,
    pub output_rule: Rule,
    pub x: f64,
    pub y: f64,
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Endpoint {
    Boundary,
    Node {
        #[serde(rename = "nodeId")]
        node_id: String,
    },
}
impl<'de> Deserialize<'de> for Endpoint {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Serde's internally tagged unit variants ignore remaining map fields even
        // with deny_unknown_fields. Use an empty struct to reject ambiguous boundary
        // objects such as {"kind":"boundary","nodeId":"deleted-node"}.
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
        enum WireEndpoint {
            Boundary {},
            Node {
                #[serde(rename = "nodeId")]
                node_id: String,
            },
        }
        Ok(match WireEndpoint::deserialize(deserializer)? {
            WireEndpoint::Boundary {} => Self::Boundary,
            WireEndpoint::Node { node_id } => Self::Node { node_id },
        })
    }
}
impl Endpoint {
    pub fn node(&self) -> Option<&str> {
        match self {
            Self::Boundary => None,
            Self::Node { node_id } => Some(node_id),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Flow {
    pub id: String,
    pub name: String,
    pub source: Endpoint,
    pub target: Endpoint,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BoundaryPoint {
    pub x: f64,
    pub y: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BoundaryPositions {
    pub input: BoundaryPoint,
    pub output: BoundaryPoint,
}
impl BoundaryPositions {
    pub fn for_nodes(nodes: &[Node]) -> Self {
        let Some(first) = nodes.first() else {
            return Self {
                input: BoundaryPoint { x: 80.0, y: 220.0 },
                output: BoundaryPoint { x: 760.0, y: 220.0 },
            };
        };
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (first.x, first.x, first.y, first.y);
        for node in nodes.iter().skip(1) {
            min_x = min_x.min(node.x);
            max_x = max_x.max(node.x);
            min_y = min_y.min(node.y);
            max_y = max_y.max(node.y);
        }
        let y = ((min_y + max_y) / 2.0).clamp(-MAX_POSITION, MAX_POSITION);
        Self {
            input: BoundaryPoint {
                x: (min_x - 280.0).clamp(-MAX_POSITION, MAX_POSITION),
                y,
            },
            output: BoundaryPoint {
                x: (max_x + 464.0).clamp(-MAX_POSITION, MAX_POSITION),
                y,
            },
        }
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Definition {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub description: String,
    pub background: String,
    pub nodes: Vec<Node>,
    pub flows: Vec<Flow>,
    pub viewport: Viewport,
    pub boundary_positions: BoundaryPositions,
}
impl<'de> Deserialize<'de> for Definition {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Only omission is legacy-compatible; explicit null or malformed positions must fail.
        fn present_positions<'de, D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<BoundaryPositions>, D::Error> {
            BoundaryPositions::deserialize(deserializer).map(Some)
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct WireDefinition {
            schema_version: u32,
            id: String,
            name: String,
            description: String,
            background: String,
            nodes: Vec<Node>,
            flows: Vec<Flow>,
            viewport: Viewport,
            #[serde(default, deserialize_with = "present_positions")]
            boundary_positions: Option<BoundaryPositions>,
        }
        let wire = WireDefinition::deserialize(deserializer)?;
        let boundary_positions = wire
            .boundary_positions
            .unwrap_or_else(|| BoundaryPositions::for_nodes(&wire.nodes));
        Ok(Self {
            schema_version: wire.schema_version,
            id: wire.id,
            name: wire.name,
            description: wire.description,
            background: wire.background,
            nodes: wire.nodes,
            flows: wire.flows,
            viewport: wire.viewport,
            boundary_positions,
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Issue {
    pub code: String,
    pub subject: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub definition: Definition,
    pub revision: u64,
    pub updated_at: i64,
    pub issues: Vec<Issue>,
}
#[derive(Debug)]
pub enum Request {
    List,
    Validate {
        definition: Definition,
    },
    Save {
        definition: Definition,
        expected_revision: u64,
    },
    Delete {
        id: String,
        expected_revision: u64,
    },
}
impl<'de> Deserialize<'de> for Request {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "operation", rename_all = "camelCase", deny_unknown_fields)]
        enum WireRequest {
            List {},
            Validate {
                definition: Definition,
            },
            Save {
                definition: Definition,
                #[serde(rename = "expectedRevision")]
                expected_revision: u64,
            },
            Delete {
                id: String,
                #[serde(rename = "expectedRevision")]
                expected_revision: u64,
            },
        }
        Ok(match WireRequest::deserialize(deserializer)? {
            WireRequest::List {} => Self::List,
            WireRequest::Validate { definition } => Self::Validate { definition },
            WireRequest::Save {
                definition,
                expected_revision,
            } => Self::Save {
                definition,
                expected_revision,
            },
            WireRequest::Delete {
                id,
                expected_revision,
            } => Self::Delete {
                id,
                expected_revision,
            },
        })
    }
}
#[derive(Debug, Serialize)]
pub struct Response {
    pub records: Vec<Record>,
    pub issues: Vec<Issue>,
}
fn issue(out: &mut Vec<Issue>, code: &str, subject: &str) {
    out.push(Issue {
        code: code.into(),
        subject: subject.into(),
    });
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.trim() == id && id.len() <= 256 && !id.chars().any(char::is_control)
}
impl Definition {
    /// Malformed or oversized definitions are rejected. Incomplete authoring choices
    /// are returned as issues so a user can save and reopen an unfinished draft.
    /// This checks structural reachability, not whether an eventual agent will select
    /// an exit or whether concurrent inputs can satisfy a runtime join.
    pub fn validate(
        &self,
        templates: &HashSet<String>,
        available_models: &HashSet<String>,
    ) -> Result<Vec<Issue>, String> {
        if self.schema_version != 1
            || !valid_id(&self.id)
            || self.nodes.len() > MAX_NODES
            || self.flows.len() > MAX_FLOWS
        {
            return Err("Unsupported workflow version, identifier or graph size".into());
        }
        if self.name.len() > MAX_NAME_BYTES
            || self.description.len() > 8_000
            || self.background.len() > MAX_TEXT_BYTES
        {
            return Err("Workflow text exceeds its size limit".into());
        }
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > 2_000_000 {
            return Err("Workflow exceeds 2 MB".into());
        }
        if !self.viewport.x.is_finite()
            || !self.viewport.y.is_finite()
            || self.viewport.x.abs() > MAX_POSITION
            || self.viewport.y.abs() > MAX_POSITION
            || !(0.25..=2.0).contains(&self.viewport.zoom)
        {
            return Err("Invalid viewport".into());
        }
        for point in [
            &self.boundary_positions.input,
            &self.boundary_positions.output,
        ] {
            if !point.x.is_finite()
                || !point.y.is_finite()
                || point.x.abs() > MAX_POSITION
                || point.y.abs() > MAX_POSITION
            {
                return Err("Invalid workflow boundary position".into());
            }
        }
        let ids: HashSet<_> = self.nodes.iter().map(|n| n.id.as_str()).collect();
        let flow_ids: HashSet<_> = self.flows.iter().map(|f| f.id.as_str()).collect();
        if ids.len() != self.nodes.len()
            || flow_ids.len() != self.flows.len()
            || ids.iter().chain(flow_ids.iter()).any(|id| !valid_id(id))
        {
            return Err("Duplicate or invalid graph identifiers".into());
        }
        for n in &self.nodes {
            if !n.x.is_finite()
                || !n.y.is_finite()
                || n.x.abs() > MAX_POSITION
                || n.y.abs() > MAX_POSITION
            {
                return Err("Invalid node position".into());
            }
            if n.template_id.as_ref().is_some_and(|id| !valid_id(id)) {
                return Err("Invalid template identifier".into());
            }
            if n.model_config_id.as_ref().is_some_and(|id| {
                id.is_empty()
                    || id.trim() != id
                    || id.len() > 512
                    || id.chars().any(char::is_control)
            }) {
                return Err("Invalid model configuration identifier".into());
            }
            if n.template_id.is_some() && n.model_config_id.is_some() {
                return Err("Template nodes cannot override their template model".into());
            }
            if n.name.len() > MAX_NAME_BYTES
                || [&n.receives, &n.task, &n.delivers]
                    .into_iter()
                    .any(|text| text.len() > MAX_TEXT_BYTES)
            {
                return Err("Node text exceeds its size limit".into());
            }
            n.input_rule.validate_shape()?;
            n.output_rule.validate_shape()?;
        }
        for f in &self.flows {
            if f.name.len() > MAX_NAME_BYTES {
                return Err("Flow name exceeds its size limit".into());
            }
            if f.source.node().is_none() && f.target.node().is_none() {
                return Err("A flow must connect a node".into());
            }
            for id in [f.source.node(), f.target.node()].into_iter().flatten() {
                if !ids.contains(id) {
                    return Err("Flow references a missing node".into());
                }
            }
        }
        let mut out = Vec::new();
        if self.name.trim().is_empty() {
            issue(&mut out, "name", &self.id);
        }
        if self.nodes.is_empty() {
            issue(&mut out, "empty", &self.id);
        }
        let entries: HashSet<_> = self
            .flows
            .iter()
            .filter(|f| f.source.node().is_none())
            .filter_map(|f| f.target.node())
            .collect();
        let exits: HashSet<_> = self
            .flows
            .iter()
            .filter(|f| f.target.node().is_none())
            .filter_map(|f| f.source.node())
            .collect();
        if entries.is_empty() {
            issue(&mut out, "entry", &self.id);
        }
        if exits.is_empty() {
            issue(&mut out, "exit", &self.id);
        }
        let reachable = self.reachable(entries, false);
        let can_exit = self.reachable(exits, true);
        for n in &self.nodes {
            if n.name.trim().is_empty() || n.task.trim().is_empty() {
                issue(&mut out, "task", &n.id);
            }
            if n.template_id
                .as_ref()
                .is_some_and(|id| !templates.contains(id))
            {
                issue(&mut out, "template", &n.id);
            }
            if n.template_id.is_none() {
                match &n.model_config_id {
                    None => issue(&mut out, "node_model", &n.id),
                    Some(id) if !available_models.contains(id) => {
                        issue(&mut out, "node_model_unavailable", &n.id)
                    }
                    Some(_) => {}
                }
            }
            if !reachable.contains(n.id.as_str()) {
                issue(&mut out, "unreachable", &n.id);
            }
            if !can_exit.contains(n.id.as_str()) {
                issue(&mut out, "noExit", &n.id);
            }
            for (rule, incoming) in [(&n.input_rule, true), (&n.output_rule, false)] {
                let flows: HashSet<_> = self
                    .flows
                    .iter()
                    .filter(|f| {
                        (if incoming {
                            f.target.node()
                        } else {
                            f.source.node()
                        }) == Some(n.id.as_str())
                    })
                    .map(|f| f.id.as_str())
                    .collect();
                if !rule.valid(&flows) {
                    issue(
                        &mut out,
                        if incoming { "inputRule" } else { "outputRule" },
                        &n.id,
                    );
                }
            }
        }
        Ok(out)
    }

    fn reachable<'a>(&'a self, mut seen: HashSet<&'a str>, reverse: bool) -> HashSet<&'a str> {
        loop {
            let count = seen.len();
            for flow in &self.flows {
                let (source, target) = if reverse {
                    (flow.target.node(), flow.source.node())
                } else {
                    (flow.source.node(), flow.target.node())
                };
                if let (Some(source), Some(target)) = (source, target) {
                    if seen.contains(source) {
                        seen.insert(target);
                    }
                }
            }
            if seen.len() == count {
                return seen;
            }
        }
    }
}
impl Rule {
    /// Presets may retain inactive custom settings while the user edits. Bound all
    /// fields, but only enforce selection semantics for the active mode in `valid`.
    fn validate_shape(&self) -> Result<(), String> {
        if self.min > MAX_FLOWS
            || self.max > MAX_FLOWS
            || self.required.len() > MAX_FLOWS
            || self.groups.len() > MAX_FLOWS
            || self.required.iter().any(|id| !valid_id(id))
        {
            return Err("Invalid or oversized flow rule".into());
        }
        let mut membership_count = self.required.len();
        for group in &self.groups {
            if !valid_id(&group.id)
                || group.min > MAX_FLOWS
                || group.max > MAX_FLOWS
                || group.flow_ids.len() > MAX_FLOWS
                || group.flow_ids.iter().any(|id| !valid_id(id))
            {
                return Err("Invalid or oversized flow rule group".into());
            }
            membership_count += group.flow_ids.len();
        }
        if membership_count > MAX_FLOWS {
            return Err("Flow rule exceeds its membership limit".into());
        }
        Ok(())
    }

    fn valid(&self, flows: &HashSet<&str>) -> bool {
        let n = flows.len();
        match self.mode {
            Mode::All => true,
            Mode::One | Mode::Any => n > 0,
            Mode::Range => self.min <= self.max && self.max <= n && self.min > 0,
            Mode::Custom => {
                let mut used = HashSet::new();
                for id in &self.required {
                    if !flows.contains(id.as_str()) || !used.insert(id.as_str()) {
                        return false;
                    }
                }
                let mut group_ids = HashSet::new();
                for g in &self.groups {
                    if !valid_id(&g.id)
                        || !group_ids.insert(&g.id)
                        || g.flow_ids.is_empty()
                        || g.min > g.max
                        || g.max > g.flow_ids.len()
                    {
                        return false;
                    }
                    for id in &g.flow_ids {
                        if !flows.contains(id.as_str()) || !used.insert(id.as_str()) {
                            return false;
                        }
                    }
                }
                // Unmentioned flows are optional. Groups are disjoint to keep authoring unambiguous.
                // At least one positive requirement prevents a zero-input trigger.
                !self.required.is_empty() || self.groups.iter().any(|g| g.min > 0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(mode: Mode) -> Rule {
        Rule {
            mode,
            min: 1,
            max: 1,
            required: Vec::new(),
            groups: Vec::new(),
        }
    }

    fn node(id: &str) -> Node {
        Node {
            id: id.into(),
            name: format!("Node {id}"),
            template_id: None,
            model_config_id: Some("model-review".into()),
            receives: "Receive the preceding result".into(),
            task: "Review the change".into(),
            delivers: "Deliver a review report".into(),
            input_rule: rule(Mode::All),
            output_rule: rule(Mode::All),
            x: 100.0,
            y: 200.0,
        }
    }

    fn endpoint(node_id: Option<&str>) -> Endpoint {
        match node_id {
            Some(id) => Endpoint::Node { node_id: id.into() },
            None => Endpoint::Boundary,
        }
    }

    fn flow(id: &str, source: Option<&str>, target: Option<&str>) -> Flow {
        Flow {
            id: id.into(),
            name: format!("Named flow {id}"),
            source: endpoint(source),
            target: endpoint(target),
        }
    }

    fn definition() -> Definition {
        Definition {
            boundary_positions: BoundaryPositions::for_nodes(&[node("review")]),
            schema_version: 1,
            id: "workflow-1".into(),
            name: "Review".into(),
            description: "Review the supplied change".into(),
            background: "Shared workflow background".into(),
            nodes: vec![node("review")],
            flows: vec![
                flow("entry", None, Some("review")),
                flow("exit", Some("review"), None),
            ],
            viewport: Viewport {
                x: 20.0,
                y: -20.0,
                zoom: 1.0,
            },
        }
    }

    fn validate(definition: &Definition) -> Vec<Issue> {
        definition
            .validate(&HashSet::new(), &HashSet::from(["model-review".into()]))
            .unwrap()
    }

    fn has_issue(issues: &[Issue], code: &str, subject: &str) -> bool {
        issues
            .iter()
            .any(|issue| issue.code == code && issue.subject == subject)
    }

    #[test]
    fn independent_node_needs_no_template_and_preserves_boundary_flows() {
        let original = definition();
        assert!(validate(&original).is_empty());
        let serialized = serde_json::to_value(&original).unwrap();
        assert_eq!(serialized["nodes"][0]["templateId"], json!(null));
        assert_eq!(
            serialized["nodes"][0]["modelConfigId"],
            json!("model-review")
        );
        assert_eq!(
            serialized["flows"][0]["source"],
            json!({"kind": "boundary"})
        );
        assert_eq!(
            serialized["flows"][0]["target"],
            json!({"kind": "node", "nodeId": "review"})
        );
        assert_eq!(serialized["flows"][1]["name"], "Named flow exit");
        let restored: Definition = serde_json::from_value(serialized.clone()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), serialized);
    }

    #[test]
    fn legacy_boundary_positions_are_derived_and_clamped_without_creating_workers() {
        let mut old = serde_json::to_value(definition()).unwrap();
        old.as_object_mut().unwrap().remove("boundaryPositions");
        let restored: Definition = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(
            restored.boundary_positions,
            BoundaryPositions {
                input: BoundaryPoint {
                    x: -180.0,
                    y: 200.0
                },
                output: BoundaryPoint { x: 564.0, y: 200.0 }
            }
        );
        assert_eq!(restored.nodes.len(), 1);
        old["nodes"] = json!([]);
        old["flows"] = json!([]);
        let empty: Definition = serde_json::from_value(old).unwrap();
        assert_eq!(
            empty.boundary_positions,
            BoundaryPositions {
                input: BoundaryPoint { x: 80.0, y: 220.0 },
                output: BoundaryPoint { x: 760.0, y: 220.0 }
            }
        );
        let mut a = node("a");
        a.x = -MAX_POSITION;
        a.y = -MAX_POSITION;
        let mut b = node("b");
        b.x = MAX_POSITION;
        b.y = MAX_POSITION;
        assert_eq!(
            BoundaryPositions::for_nodes(&[a, b]),
            BoundaryPositions {
                input: BoundaryPoint {
                    x: -MAX_POSITION,
                    y: 0.0
                },
                output: BoundaryPoint {
                    x: MAX_POSITION,
                    y: 0.0
                }
            }
        );
    }

    #[test]
    fn explicit_boundary_positions_are_strict_and_stay_within_layout_limits() {
        let original = serde_json::to_value(definition()).unwrap();
        for position in [
            json!(null),
            json!({}),
            json!({"input":{"x":0,"y":0}}),
            json!({"input":{"x":0,"y":0},"output":{"x":0,"y":0},"extra":true}),
            json!({"input":{"x":0,"y":0,"nodeId":"worker"},"output":{"x":0,"y":0}}),
        ] {
            let mut wire = original.clone();
            wire["boundaryPositions"] = position;
            assert!(serde_json::from_value::<Definition>(wire).is_err());
        }
        let mut graph = definition();
        graph.boundary_positions.output.x = 100001.0;
        assert!(graph.validate(&HashSet::new(), &HashSet::new()).is_err());
        graph.boundary_positions.output.x = 0.0;
        graph.boundary_positions.input.y = f64::INFINITY;
        assert!(graph.validate(&HashSet::new(), &HashSet::new()).is_err());
    }

    #[test]
    fn legacy_v1_nodes_default_to_an_unselected_model_without_rejecting_the_graph() {
        let mut old = serde_json::to_value(definition()).unwrap();
        old["nodes"][0]
            .as_object_mut()
            .unwrap()
            .remove("modelConfigId");
        let restored: Definition = serde_json::from_value(old).unwrap();
        assert!(restored.nodes[0].model_config_id.is_none());
        assert_eq!(
            validate(&restored),
            vec![Issue {
                code: "node_model".into(),
                subject: "review".into()
            }]
        );
        let mut template = restored;
        template.nodes[0].template_id = Some("template-review".into());
        assert!(template
            .validate(&HashSet::from(["template-review".into()]), &HashSet::new())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn model_choices_use_the_authoritative_available_set_and_template_overrides_are_forbidden() {
        let mut graph = definition();
        assert!(validate(&graph).is_empty());
        let issues = graph.validate(&HashSet::new(), &HashSet::new()).unwrap();
        assert_eq!(
            issues,
            vec![Issue {
                code: "node_model_unavailable".into(),
                subject: "review".into()
            }]
        );
        graph.nodes[0].template_id = Some("template-review".into());
        assert!(graph
            .validate(
                &HashSet::from(["template-review".into()]),
                &HashSet::from(["model-review".into()])
            )
            .unwrap_err()
            .contains("override"));
        graph.nodes[0].template_id = None;
        for invalid in [
            String::new(),
            " padded".into(),
            "line\nbreak".into(),
            "x".repeat(513),
        ] {
            graph.nodes[0].model_config_id = Some(invalid);
            assert!(graph.validate(&HashSet::new(), &HashSet::new()).is_err());
        }
    }

    #[test]
    fn shared_protocol_fixture_round_trips_and_validates() {
        let mut fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../packages/protocol/fixtures/workflow-definition-v1.json"
        ))
        .unwrap();
        let definition: Definition = serde_json::from_value(fixture.clone()).unwrap();
        assert_eq!(
            validate(&definition)
                .iter()
                .map(|issue| issue.code.as_str())
                .collect::<Vec<_>>(),
            vec!["node_model", "node_model"]
        );
        // JSON has one number type; normalize its layout values to Rust's f64
        // representation before comparing the serialized wire contract.
        for node in fixture["nodes"].as_array_mut().unwrap() {
            for field in ["x", "y"] {
                node[field] = json!(node[field].as_f64().unwrap());
            }
        }
        for field in ["x", "y", "zoom"] {
            fixture["viewport"][field] = json!(fixture["viewport"][field].as_f64().unwrap());
        }
        for side in ["input", "output"] {
            for field in ["x", "y"] {
                fixture["boundaryPositions"][side][field] =
                    json!(fixture["boundaryPositions"][side][field].as_f64().unwrap());
            }
        }
        assert_eq!(serde_json::to_value(definition).unwrap(), fixture);
    }

    #[test]
    fn incomplete_authoring_and_missing_template_are_saveable_draft_issues() {
        let mut draft = definition();
        draft.name.clear();
        draft.nodes[0].name.clear();
        draft.nodes[0].task.clear();
        draft.nodes[0].template_id = Some("deleted-template".into());
        draft.nodes[0].model_config_id = None;
        let issues = validate(&draft);
        assert!(has_issue(&issues, "name", "workflow-1"));
        assert!(has_issue(&issues, "task", "review"));
        assert!(has_issue(&issues, "template", "review"));
        let templates = HashSet::from(["deleted-template".to_owned()]);
        assert!(!has_issue(
            &draft
                .validate(&templates, &HashSet::from(["model-review".into()]))
                .unwrap(),
            "template",
            "review"
        ));
    }

    #[test]
    fn empty_and_disconnected_graphs_remain_editable_drafts() {
        let mut draft = definition();
        draft.nodes.clear();
        draft.flows.clear();
        let issues = validate(&draft);
        for code in ["empty", "entry", "exit"] {
            assert!(has_issue(&issues, code, "workflow-1"));
        }
        draft.nodes.push(node("isolated"));
        let issues = validate(&draft);
        assert!(has_issue(&issues, "unreachable", "isolated"));
        assert!(has_issue(&issues, "noExit", "isolated"));
        // A zero-degree ALL rule is not itself malformed; reachability diagnoses it.
        assert!(!has_issue(&issues, "inputRule", "isolated"));
    }

    #[test]
    fn cycles_with_an_exit_and_explicit_boundary_entry_are_supported() {
        let mut graph = definition();
        graph.nodes.push(node("develop"));
        graph.nodes[0].input_rule = rule(Mode::One);
        graph.nodes[0].output_rule = rule(Mode::One);
        graph
            .flows
            .push(flow("rework", Some("review"), Some("develop")));
        graph
            .flows
            .push(flow("retry", Some("develop"), Some("review")));
        assert!(validate(&graph).is_empty());
        graph.flows.retain(|flow| flow.id != "exit");
        let issues = validate(&graph);
        assert!(has_issue(&issues, "exit", "workflow-1"));
        assert!(has_issue(&issues, "noExit", "review"));
        assert!(has_issue(&issues, "noExit", "develop"));
    }

    #[test]
    fn no_input_degree_does_not_implicitly_make_a_node_an_entry() {
        let mut graph = definition();
        graph.flows.retain(|flow| flow.id != "entry");
        let issues = validate(&graph);
        assert!(has_issue(&issues, "entry", "workflow-1"));
        assert!(has_issue(&issues, "unreachable", "review"));
    }

    #[test]
    fn multiple_boundary_inputs_and_outputs_are_preserved() {
        let mut graph = definition();
        graph.flows.push(flow("entry-2", None, Some("review")));
        graph.flows.push(flow("exit-2", Some("review"), None));
        graph.nodes[0].input_rule = rule(Mode::Any);
        graph.nodes[0].output_rule = rule(Mode::One);
        assert!(validate(&graph).is_empty());
        assert_eq!(graph.flows.len(), 4);
    }

    #[test]
    fn malformed_topology_is_a_hard_error_not_a_draft_issue() {
        let mut graph = definition();
        graph
            .flows
            .push(flow("broken", Some("missing"), Some("review")));
        assert!(graph
            .validate(&HashSet::new(), &HashSet::from(["model-review".into()]))
            .unwrap_err()
            .contains("missing node"));
        graph.flows.pop();
        graph.flows.push(flow("broken", None, None));
        assert!(graph
            .validate(&HashSet::new(), &HashSet::from(["model-review".into()]))
            .unwrap_err()
            .contains("connect a node"));
        graph.flows.pop();
        graph.nodes.push(node("review"));
        assert!(graph
            .validate(&HashSet::new(), &HashSet::from(["model-review".into()]))
            .unwrap_err()
            .contains("identifiers"));
        graph.nodes.pop();
        graph.flows.push(flow("exit", Some("review"), None));
        assert!(graph
            .validate(&HashSet::new(), &HashSet::from(["model-review".into()]))
            .unwrap_err()
            .contains("identifiers"));
    }

    #[test]
    fn presets_and_ranges_validate_cardinality() {
        let flows = HashSet::from(["a", "b", "c"]);
        for mode in [Mode::All, Mode::One, Mode::Any] {
            assert!(rule(mode).valid(&flows));
        }
        assert!(!rule(Mode::One).valid(&HashSet::new()));
        assert!(!rule(Mode::Any).valid(&HashSet::new()));
        for (min, max, expected) in [
            (1, 3, true),
            (2, 2, true),
            (0, 2, false),
            (3, 2, false),
            (1, 4, false),
        ] {
            let mut rule = rule(Mode::Range);
            rule.min = min;
            rule.max = max;
            assert_eq!(rule.valid(&flows), expected, "range {min}..={max}");
        }
    }

    #[test]
    fn custom_rules_support_required_optional_and_disjoint_choice_groups() {
        let flows = HashSet::from(["required", "b", "c", "optional"]);
        let mut custom = rule(Mode::Custom);
        custom.required = vec!["required".into()];
        custom.groups = vec![Group {
            id: "choice".into(),
            flow_ids: vec!["b".into(), "c".into()],
            min: 1,
            max: 1,
        }];
        assert!(custom.valid(&flows));
        // An unmentioned flow remains optional. Required-only rules are valid too.
        custom.groups.clear();
        assert!(custom.valid(&flows));
        custom.required.clear();
        assert!(!custom.valid(&flows));
        custom.groups = vec![Group {
            id: "choice".into(),
            flow_ids: vec!["b".into(), "c".into()],
            min: 1,
            max: 2,
        }];
        assert!(custom.valid(&flows));
    }

    #[test]
    fn custom_rules_reject_conflicting_missing_duplicate_or_impossible_members() {
        let flows = HashSet::from(["a", "b", "c"]);
        let mut base = rule(Mode::Custom);
        base.required = vec!["a".into()];
        base.groups = vec![Group {
            id: "g".into(),
            flow_ids: vec!["b".into(), "c".into()],
            min: 1,
            max: 1,
        }];
        let mut invalid = Vec::new();
        let mut r = base.clone();
        r.required.push("a".into());
        invalid.push(r);
        let mut r = base.clone();
        r.required.push("missing".into());
        invalid.push(r);
        let mut r = base.clone();
        r.groups[0].flow_ids.push("a".into());
        invalid.push(r);
        let mut r = base.clone();
        r.groups[0].flow_ids.push("b".into());
        invalid.push(r);
        let mut r = base.clone();
        r.groups[0].flow_ids.push("missing".into());
        invalid.push(r);
        let mut r = base.clone();
        r.groups.push(r.groups[0].clone());
        invalid.push(r);
        let mut r = base.clone();
        r.groups[0].max = 3;
        invalid.push(r);
        let mut r = base.clone();
        r.groups[0].min = 2;
        invalid.push(r);
        let mut r = base.clone();
        r.groups[0].flow_ids.clear();
        invalid.push(r);
        for (index, r) in invalid.into_iter().enumerate() {
            assert!(!r.valid(&flows), "invalid custom rule case {index}");
        }
    }

    #[test]
    fn active_rule_can_only_reference_flows_incident_on_its_own_side() {
        let mut graph = definition();
        graph.nodes[0].input_rule.mode = Mode::Custom;
        graph.nodes[0].input_rule.required = vec!["exit".into()];
        assert!(has_issue(&validate(&graph), "inputRule", "review"));
        graph.nodes[0].input_rule.required = vec!["entry".into()];
        assert!(validate(&graph).is_empty());
        graph.nodes[0].output_rule.mode = Mode::Range;
        graph.nodes[0].output_rule.max = 2;
        assert!(has_issue(&validate(&graph), "outputRule", "review"));
    }

    #[test]
    fn inactive_rule_settings_are_retained_but_still_resource_bounded() {
        let mut graph = definition();
        graph.nodes[0].input_rule.required = vec!["previously-connected-flow".into()];
        assert!(validate(&graph).is_empty());
        graph.nodes[0].input_rule.required = vec!["a".repeat(257)];
        assert!(graph
            .validate(&HashSet::new(), &HashSet::from(["model-review".into()]))
            .is_err());
        graph.nodes[0].input_rule.required.clear();
        graph.nodes[0].input_rule.max = usize::MAX;
        assert!(graph
            .validate(&HashSet::new(), &HashSet::from(["model-review".into()]))
            .is_err());
    }

    #[test]
    fn unknown_schema_invalid_identifiers_and_nonfinite_layout_are_rejected() {
        let mut graph = definition();
        graph.schema_version = 2;
        assert!(graph
            .validate(&HashSet::new(), &HashSet::from(["model-review".into()]))
            .is_err());
        graph.schema_version = 1;
        for id in ["", " leading-space", "newline\ninside"] {
            graph.id = id.into();
            assert!(graph
                .validate(&HashSet::new(), &HashSet::from(["model-review".into()]))
                .is_err());
        }
        graph.id = "valid".into();
        graph.nodes[0].x = f64::NAN;
        assert!(graph
            .validate(&HashSet::new(), &HashSet::from(["model-review".into()]))
            .is_err());
        graph.nodes[0].x = 0.0;
        graph.viewport.zoom = f64::INFINITY;
        assert!(graph
            .validate(&HashSet::new(), &HashSet::from(["model-review".into()]))
            .is_err());
    }

    #[test]
    fn text_and_graph_size_limits_are_enforced() {
        let mut graph = definition();
        graph.background = "x".repeat(MAX_TEXT_BYTES + 1);
        assert!(graph
            .validate(&HashSet::new(), &HashSet::from(["model-review".into()]))
            .is_err());
        graph.background.clear();
        graph.flows[0].name = "x".repeat(MAX_NAME_BYTES + 1);
        assert!(graph
            .validate(&HashSet::new(), &HashSet::from(["model-review".into()]))
            .is_err());
        graph.flows[0].name.clear();
        graph.nodes = (0..=MAX_NODES).map(|i| node(&format!("n{i}"))).collect();
        assert!(graph
            .validate(&HashSet::new(), &HashSet::from(["model-review".into()]))
            .is_err());
    }

    #[test]
    fn wire_schema_rejects_ambiguous_boundaries_and_non_integer_counts() {
        assert!(matches!(
            serde_json::from_value::<Request>(json!({"operation": "list"})).unwrap(),
            Request::List
        ));
        assert!(serde_json::from_value::<Request>(json!({
            "operation": "list", "definition": definition()
        }))
        .is_err());
        let mut value = serde_json::to_value(definition()).unwrap();
        value["flows"][0]["source"] = json!({"kind": "boundary", "nodeId": "review"});
        assert!(serde_json::from_value::<Definition>(value).is_err());
        for count in [json!(-1), json!(1.5), json!("2"), json!(1e30)] {
            let mut value = serde_json::to_value(definition()).unwrap();
            value["nodes"][0]["inputRule"]["min"] = count;
            assert!(serde_json::from_value::<Definition>(value).is_err());
        }
        let mut value = serde_json::to_value(definition()).unwrap();
        value["script"] = json!("not executable");
        assert!(serde_json::from_value::<Definition>(value).is_err());
    }
}
