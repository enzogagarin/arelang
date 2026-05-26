#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    HttpResponseOk,
    HttpResponseCreated,
    RequestJson,
    ValidateEmail,
    ValidateLength,
    ContextParam,
    StateUsersInsert,
    StateUsersGet,
}

impl Builtin {
    #[must_use]
    pub const fn callee(self) -> &'static str {
        match self {
            Self::HttpResponseOk => "Http.Response.ok",
            Self::HttpResponseCreated => "Http.Response.created",
            Self::RequestJson => "req.json",
            Self::ValidateEmail => "validate.email",
            Self::ValidateLength => "validate.length",
            Self::ContextParam => "ctx.param",
            Self::StateUsersInsert => "ctx.state.users.insert",
            Self::StateUsersGet => "ctx.state.users.get",
        }
    }
}

#[must_use]
pub fn builtin_by_callee(callee: &str) -> Option<Builtin> {
    match callee {
        "Http.Response.ok" => Some(Builtin::HttpResponseOk),
        "Http.Response.created" => Some(Builtin::HttpResponseCreated),
        "req.json" => Some(Builtin::RequestJson),
        "validate.email" => Some(Builtin::ValidateEmail),
        "validate.length" => Some(Builtin::ValidateLength),
        "ctx.param" => Some(Builtin::ContextParam),
        "ctx.state.users.insert" => Some(Builtin::StateUsersInsert),
        "ctx.state.users.get" => Some(Builtin::StateUsersGet),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Builtin, builtin_by_callee};

    #[test]
    fn resolves_builtin_callees() {
        for builtin in [
            Builtin::HttpResponseOk,
            Builtin::HttpResponseCreated,
            Builtin::RequestJson,
            Builtin::ValidateEmail,
            Builtin::ValidateLength,
            Builtin::ContextParam,
            Builtin::StateUsersInsert,
            Builtin::StateUsersGet,
        ] {
            assert_eq!(builtin_by_callee(builtin.callee()), Some(builtin));
        }
    }
}
