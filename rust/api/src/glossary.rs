use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    Mandatory,
    #[default]
    Default,
    Guideline,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Draft,
    Approved,
    Deprecated,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LanguageTerm {
    pub preferred: String,
    #[serde(default)]
    pub allowed: Vec<String>,
    #[serde(default)]
    pub deprecated: Vec<String>,
    #[serde(default)]
    pub usage: String,
    #[serde(default)]
    pub examples: Vec<String>,
    #[serde(default)]
    pub reviewed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Concept {
    pub name: String,
    pub definition: String,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub rule: Rule,
    #[serde(default)]
    pub status: Status,
    #[serde(default)]
    pub languages: BTreeMap<String, LanguageTerm>,
    #[serde(default)]
    pub related: Vec<String>,
    #[serde(default)]
    pub specialization_reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Guideline {
    pub key: String,
    pub text: String,
    #[serde(default)]
    pub rule: Rule,
    #[serde(default)]
    pub specialization_reason: String,
}

macro_rules! request {
    ($name:ident { $($fields:tt)* }) => {
        #[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            pub path: Option<String>,
            pub repository_id: Option<String>,
            $($fields)*
        }
    };
}

request!(Reference {});
request!(List {
    pub revision: Option<u32>,
    pub query: Option<String>,
    pub language: Option<String>,
    pub status: Option<Status>,
    pub origin: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub expected_revision: Option<u32>,
});
request!(Get {
    pub concept_id: String,
    pub revision: Option<u32>,
});
request!(Save {
    pub expected_revision: u32,
    pub concept_id: Option<String>,
    pub concept: Concept,
});
request!(Configure {
    pub expected_revision: u32,
    pub baseline_revision: Option<u32>,
    pub languages: Vec<String>,
    pub guidelines: Vec<Guideline>,
});
request!(Inherit {
    pub expected_revision: u32,
    pub concept_id: String,
});
request!(History {
    pub concept_id: Option<String>,
    pub before_revision: Option<u32>,
    pub limit: Option<usize>,
});
request!(Check {
    pub expected_revision: Option<u32>,
    pub usages: Vec<Usage>,
});

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub concept_id: String,
    pub language: String,
    pub term: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Profile {
    pub repository_id: Option<String>,
    pub display_name: String,
    pub revision: u32,
    pub baseline_revision: u32,
    pub latest_shared_revision: u32,
    pub adoption_needed: bool,
    pub languages: Vec<String>,
    pub available_languages: Vec<String>,
    pub local_guidelines: Vec<Guideline>,
    pub guidelines: Vec<EffectiveGuideline>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct EffectiveGuideline {
    pub guideline: Guideline,
    pub origin: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Entry {
    pub concept_id: String,
    pub concept: Concept,
    pub origin: String,
    pub inherited_from: Option<u32>,
    pub shared_concept: Option<Concept>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Page {
    pub profile: Profile,
    pub entries: Vec<Entry>,
    pub total: usize,
    pub next_offset: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Detail {
    pub profile: Profile,
    pub entry: Entry,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Mutation {
    pub revision: u32,
    pub concept_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Revision {
    pub revision: u32,
    pub kind: String,
    pub concept_id: Option<String>,
    pub summary: String,
    pub actor: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct HistoryPage {
    pub revisions: Vec<Revision>,
    pub next_before_revision: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CheckFinding {
    pub index: usize,
    pub code: String,
    pub preferred: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CheckResult {
    pub profile: Profile,
    pub valid: bool,
    pub findings: Vec<CheckFinding>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImpactRequest {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProjectImpact {
    pub repository_id: String,
    pub display_name: String,
    pub baseline_revision: u32,
    pub configured: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Impact {
    pub shared_revision: u32,
    pub projects: Vec<ProjectImpact>,
    pub total: usize,
    pub next_offset: Option<usize>,
}
