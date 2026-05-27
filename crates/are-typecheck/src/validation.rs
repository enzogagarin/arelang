use super::{TypeChecker, path_is};
use are_ast::{Field, FieldValidation, TypeDecl, TypeExpr};
use are_diagnostics::Diagnostic;
use std::collections::HashSet;

impl TypeChecker<'_> {
    pub(super) fn check_field_validations(&mut self, fields: &[Field]) {
        for field in fields {
            self.check_validations(
                &field.ty,
                &field.validations,
                "field",
                &field.name,
                &format!(
                    "`{}` must be `String`, `Text`, or an alias to a string type",
                    field.name
                ),
            );
        }
    }

    pub(super) fn check_type_validations(&mut self, decl: &TypeDecl) {
        self.check_validations(
            &decl.aliased,
            &decl.validations,
            "type",
            &decl.name,
            &format!(
                "`{}` must alias `String`, `Text`, or another string-like type",
                decl.name
            ),
        );
    }

    fn check_validations(
        &mut self,
        ty: &TypeExpr,
        validations: &[FieldValidation],
        target_kind: &str,
        target_name: &str,
        type_help: &str,
    ) {
        let mut email_validation = None;
        let mut length_validation = None;

        for validation in validations {
            match validation {
                FieldValidation::Email { range } => {
                    if let Some(previous) = email_validation.replace(*range) {
                        self.duplicate(
                            "E_TYPE_0012",
                            *range,
                            format!(
                                "duplicate {target_kind} validation `validate.email` on `{target_name}`"
                            ),
                            previous,
                        );
                    }

                    if !self.is_string_like_type_expr(ty) {
                        self.diagnostics.push(Diagnostic::error(
                            "E_TYPE_0010",
                            &self.file,
                            *range,
                            format!(
                                "{target_kind} validation `validate.email` expects a string-like type"
                            ),
                            type_help.to_string(),
                        ));
                    }
                }
                FieldValidation::Length { min, max, range } => {
                    if let Some(previous) = length_validation.replace(*range) {
                        self.duplicate(
                            "E_TYPE_0012",
                            *range,
                            format!(
                                "duplicate {target_kind} validation `validate.length` on `{target_name}`"
                            ),
                            previous,
                        );
                    }

                    if !self.is_string_like_type_expr(ty) {
                        self.diagnostics.push(Diagnostic::error(
                            "E_TYPE_0010",
                            &self.file,
                            *range,
                            format!(
                                "{target_kind} validation `validate.length` expects a string-like type"
                            ),
                            type_help.to_string(),
                        ));
                    }

                    if *min < 0 || *max < 0 || min > max {
                        self.diagnostics.push(Diagnostic::error(
                            "E_TYPE_0011",
                            &self.file,
                            *range,
                            "invalid `validate.length` bounds",
                            "`min` and `max` must be non-negative and `min` must be less than or equal to `max`",
                        ));
                    }
                }
            }
        }
    }

    fn is_string_like_type_expr(&self, ty: &TypeExpr) -> bool {
        matches!(
            self.type_expr_primitive_root(ty, &mut HashSet::new())
                .as_deref(),
            Some("String" | "Text")
        )
    }

    fn type_expr_primitive_root(
        &self,
        ty: &TypeExpr,
        visited: &mut HashSet<String>,
    ) -> Option<String> {
        match ty {
            TypeExpr::Path { path } => {
                if path.segments.len() != 1 {
                    return None;
                }

                let name = &path.segments[0];
                if is_primitive_type_name(name) {
                    return Some(name.clone());
                }

                if !visited.insert(name.clone()) {
                    return None;
                }

                let alias = self.types.get(name)?;
                self.type_expr_primitive_root(&alias.aliased, visited)
            }
            TypeExpr::Generic { base, args, .. }
                if path_is(base, &["Option"]) && args.len() == 1 =>
            {
                self.type_expr_primitive_root(&args[0], visited)
            }
            TypeExpr::Option { inner, .. } => self.type_expr_primitive_root(inner, visited),
            TypeExpr::Generic { .. } => None,
        }
    }
}

fn is_primitive_type_name(name: &str) -> bool {
    matches!(
        name,
        "String" | "Text" | "Bool" | "Int" | "I64" | "U64" | "F64"
    )
}
