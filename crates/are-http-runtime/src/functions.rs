use crate::schemas::RuntimeSchemas;
use are_ast::{FunctionDecl, Item};
use are_project::CheckedFile;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub(crate) struct RuntimeFunctions {
    pub(crate) functions: HashMap<String, FunctionDecl>,
    pub(crate) schemas: RuntimeSchemas,
}

impl RuntimeFunctions {
    pub(crate) fn from_modules(modules: &[CheckedFile]) -> Self {
        let functions = modules
            .iter()
            .flat_map(|file| file.module.items.iter())
            .filter_map(|item| {
                if let Item::Function(function) = item {
                    Some((function.name.clone(), function.clone()))
                } else {
                    None
                }
            })
            .collect();

        Self {
            functions,
            schemas: RuntimeSchemas::from_modules(modules),
        }
    }

    pub(crate) fn get(&self, name: &str) -> Option<&FunctionDecl> {
        self.functions.get(name)
    }
}
