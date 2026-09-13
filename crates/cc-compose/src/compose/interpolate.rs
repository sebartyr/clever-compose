//! Interpolation de l'arbre brut (compose-go `interpolation`), puis
//! conversion de type sur les chemins qui attendent un nombre ou un
//! booléen (`interpolateTypeCastMapping` du loader).

use indexmap::IndexMap;

use super::node::{Node, Path, Value};
use super::template::{self, Lookup};
use crate::diagnostic::Diagnostic;

pub fn interpolate(node: Node, lookup: Lookup<'_>, diags: &mut Vec<Diagnostic>) -> Node {
    at(node, &Path::root(), lookup, diags)
}

fn at(node: Node, p: &Path, lookup: Lookup<'_>, diags: &mut Vec<Diagnostic>) -> Node {
    match node.value {
        Value::Str(s) => {
            let value = match template::substitute(&s, lookup) {
                Ok(v) => v,
                Err(e) => {
                    diags.push(Diagnostic::error("interpolation", format!("{p} : {e}")).at(p, node.span));
                    return Node { span: node.span, tag: node.tag, value: Value::Str(s) };
                }
            };
            let value = match cast_for(p) {
                None => Value::Str(value),
                Some(cast) => match cast(&value) {
                    Ok(v) => v,
                    Err(e) => {
                        diags.push(Diagnostic::error("interpolation-type", format!("{p} : conversion impossible : {e}")).at(p, node.span));
                        Value::Str(value)
                    }
                },
            };
            Node { span: node.span, tag: node.tag, value }
        }
        Value::Map(m) => {
            let mut out = IndexMap::with_capacity(m.len());
            for (k, v) in m {
                let t = at(v, &p.child(&k), lookup, diags);
                out.insert(k, t);
            }
            Node { span: node.span, tag: node.tag, value: Value::Map(out) }
        }
        Value::Seq(s) => {
            let mut out = Vec::with_capacity(s.len());
            for (i, v) in s.into_iter().enumerate() {
                out.push(at(v, &p.index(i), lookup, diags));
            }
            Node { span: node.span, tag: node.tag, value: Value::Seq(out) }
        }
        _ => node,
    }
}

type Cast = fn(&str) -> Result<Value, String>;

fn cast_for(p: &Path) -> Option<Cast> {
    const INT: &[&str] = &[
        "services.*.cpu_count",
        "services.*.cpu_period",
        "services.*.cpu_quota",
        "services.*.cpu_rt_period",
        "services.*.cpu_rt_runtime",
        "services.*.cpu_shares",
        "services.*.deploy.replicas",
        "services.*.deploy.update_config.parallelism",
        "services.*.deploy.rollback_config.parallelism",
        "services.*.deploy.restart_policy.max_attempts",
        "services.*.deploy.placement.max_replicas_per_node",
        "services.*.healthcheck.retries",
        "services.*.oom_score_adj",
        "services.*.pids_limit",
        "services.*.ports.[].target",
        "services.*.scale",
        "services.*.ulimits.*",
        "services.*.ulimits.*.hard",
        "services.*.ulimits.*.soft",
    ];
    const FLOAT: &[&str] = &["services.*.cpu_percent", "services.*.cpus", "services.*.deploy.update_config.max_failure_ratio", "services.*.deploy.rollback_config.max_failure_ratio"];
    const BOOL: &[&str] = &[
        "services.*.init",
        "services.*.depends_on.*.required",
        "services.*.depends_on.*.restart",
        "services.*.healthcheck.disable",
        "services.*.oom_kill_disable",
        "services.*.privileged",
        "services.*.read_only",
        "services.*.stdin_open",
        "services.*.tty",
        "services.*.volumes.[].read_only",
        "services.*.volumes.[].volume.nocopy",
        "networks.*.external",
        "networks.*.internal",
        "networks.*.attachable",
        "networks.*.enable_ipv4",
        "networks.*.enable_ipv6",
        "volumes.*.external",
        "secrets.*.external",
        "configs.*.external",
    ];
    if INT.iter().any(|pat| p.matches(pat)) {
        return Some(|s| s.trim().parse::<i64>().map(Value::Int).map_err(|e| e.to_string()));
    }
    if FLOAT.iter().any(|pat| p.matches(pat)) {
        return Some(|s| s.trim().parse::<f64>().map(Value::Float).map_err(|e| e.to_string()));
    }
    if BOOL.iter().any(|pat| p.matches(pat)) {
        return Some(|s| match s.to_lowercase().as_str() {
            "true" | "y" | "yes" | "on" => Ok(Value::Bool(true)),
            "false" | "n" | "no" | "off" => Ok(Value::Bool(false)),
            other => Err(format!("booléen invalide : {other}")),
        });
    }
    None
}
