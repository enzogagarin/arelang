#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    HttpResponseOk,
    HttpResponseCreated,
    HttpResponseError,
    RequestJson,
    ValidateEmail,
    ValidateLength,
    ContextParam,
}

impl Builtin {
    #[must_use]
    pub const fn callee(self) -> &'static str {
        match self {
            Self::HttpResponseOk => "Http.Response.ok",
            Self::HttpResponseCreated => "Http.Response.created",
            Self::HttpResponseError => "Http.Response.error",
            Self::RequestJson => "req.json",
            Self::ValidateEmail => "validate.email",
            Self::ValidateLength => "validate.length",
            Self::ContextParam => "ctx.param",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbOperation {
    Insert,
    Get,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbCall<'a> {
    pub collection: &'a str,
    pub operation: DbOperation,
}

#[must_use]
pub fn builtin_by_callee(callee: &str) -> Option<Builtin> {
    match callee {
        "Http.Response.ok" => Some(Builtin::HttpResponseOk),
        "Http.Response.created" => Some(Builtin::HttpResponseCreated),
        "Http.Response.error" => Some(Builtin::HttpResponseError),
        "req.json" => Some(Builtin::RequestJson),
        "validate.email" => Some(Builtin::ValidateEmail),
        "validate.length" => Some(Builtin::ValidateLength),
        "ctx.param" => Some(Builtin::ContextParam),
        _ => None,
    }
}

#[must_use]
pub fn db_call_by_callee(callee: &str) -> Option<DbCall<'_>> {
    let mut parts = callee.split('.');
    let ctx = parts.next()?;
    let db = parts.next()?;
    let collection = parts.next()?;
    let operation = parts.next()?;
    if parts.next().is_some() || ctx != "ctx" || db != "db" {
        return None;
    }

    let operation = match operation {
        "insert" => DbOperation::Insert,
        "get" => DbOperation::Get,
        _ => return None,
    };

    Some(DbCall {
        collection,
        operation,
    })
}

#[must_use]
pub fn collection_name_for_model(model_name: &str) -> String {
    let base = lower_snake(model_name);
    if base.ends_with('s') {
        format!("{base}es")
    } else {
        format!("{base}s")
    }
}

fn lower_snake(value: &str) -> String {
    let mut output = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                output.push('_');
            }
            output.push(ch.to_ascii_lowercase());
        } else {
            output.push(ch);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{
        Builtin, DbOperation, builtin_by_callee, collection_name_for_model, db_call_by_callee,
    };

    #[test]
    fn resolves_builtin_callees() {
        for builtin in [
            Builtin::HttpResponseOk,
            Builtin::HttpResponseCreated,
            Builtin::HttpResponseError,
            Builtin::RequestJson,
            Builtin::ValidateEmail,
            Builtin::ValidateLength,
            Builtin::ContextParam,
        ] {
            assert_eq!(builtin_by_callee(builtin.callee()), Some(builtin));
        }
    }

    #[test]
    fn resolves_model_db_callees() {
        let call = db_call_by_callee("ctx.db.users.insert").expect("db call");
        assert_eq!(call.collection, "users");
        assert_eq!(call.operation, DbOperation::Insert);
        assert!(db_call_by_callee("ctx.state.users.insert").is_none());
        assert_eq!(collection_name_for_model("User"), "users");
        assert_eq!(collection_name_for_model("BlogPost"), "blog_posts");
    }
}
