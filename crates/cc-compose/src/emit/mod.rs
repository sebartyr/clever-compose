//! Émetteurs : sérialisations du plan pour reprendre la main (CLAUDE.md
//! §10). Terraform (réconciliable) et script clever-tools (bootstrap).

pub mod export;
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
pub fn member_parts(plan: &Plan, member: &str) -> Option<(String, String)> {
    plan.resources.iter().find_map(|r| match r {
        Resource::Membership(m) if m.id.0 == member => {
            let service = m.member.0.strip_prefix("app/")?.to_string();
            let net = m.group.0.strip_prefix("ng/")?.to_string();
            Some((service, net))
        }
        _ => None,
    })
}
