use super::{Expr, Field, Query};

const ANY_COLUMNS: &str = "{from_text subject body}";

impl Query {
    #[must_use]
    pub fn to_fts5(&self) -> Option<String> {
        fts(self.expr()).map(|(s, _)| s)
    }
}

fn fts(e: &Expr) -> Option<(String, bool)> {
    match e {
        Expr::Term(t) if !t.in_index() => None,
        Expr::Term(t) => {
            let column = match t.field {
                Field::Any => ANY_COLUMNS,
                Field::From => "from_text",
                Field::To => "to_text",
                Field::Cc => "cc_text",
                Field::Subject => "subject",
                Field::Bcc | Field::Label => return None,
            };
            let literal = fts_string(&t.value)?;
            let star = if t.prefix { "*" } else { "" };
            Some((format!("{column} : {literal}{star}"), false))
        }
        Expr::IsList
        | Expr::Not(_)
        | Expr::HasAttachment
        | Expr::Snoozed
        | Expr::Anywhere
        | Expr::Date { .. }
        | Expr::Unsupported(_) => None,
        Expr::And(xs) => join(xs.iter().filter_map(fts).collect(), " AND "),
        Expr::Or(xs) => join(xs.iter().map(fts).collect::<Option<_>>()?, " OR "),
    }
}

fn join(parts: Vec<(String, bool)>, op: &str) -> Option<(String, bool)> {
    if parts.len() <= 1 {
        return parts.into_iter().next();
    }
    let joined = parts
        .into_iter()
        .map(|(s, compound)| if compound { format!("({s})") } else { s })
        .collect::<Vec<_>>()
        .join(op);
    Some((joined, true))
}

fn fts_string(value: &str) -> Option<String> {
    if !value.chars().any(char::is_alphanumeric) {
        return None;
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\"\""),
            _ if c.is_control() => out.push(' '),
            _ => out.push(c),
        }
    }
    out.push('"');
    Some(out)
}
