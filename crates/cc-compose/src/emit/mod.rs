//! Émetteurs : sérialisations du plan pour reprendre la main (CLAUDE.md
//! §10). Terraform (réconciliable) et script clever-tools (bootstrap).

pub mod export;
pub mod hcl;
pub mod script;
pub mod terraform;

use cc_compose_model::plan::{Plan, Resource};

/// Nom de ressource valide en Terraform et en shell : lettres, chiffres et
/// `_`, jamais un chiffre en tête.
pub fn ident(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// Le service d'un membre `member/<service>@<réseau>` et son réseau.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
    App,
    Addon,
}

/// Nature, service et réseau d'un membre (`member/<service>@<réseau>`).
pub fn member_parts(plan: &Plan, member: &str) -> Option<(MemberKind, String, String)> {
    plan.resources.iter().find_map(|r| match r {
        Resource::Membership(m) if m.id.0 == member => {
            let net = m.group.0.strip_prefix("ng/")?.to_string();
            if let Some(s) = m.member.0.strip_prefix("app/") {
                return Some((MemberKind::App, s.to_string(), net));
            }
            m.member.0.strip_prefix("addon/").map(|s| (MemberKind::Addon, s.to_string(), net))
        }
        _ => None,
    })
}
