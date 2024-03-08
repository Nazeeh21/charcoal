mod assembly;
mod contracts;
mod enums;
mod expressions;
mod functions;
mod import_directives;
mod statements;
mod storage;
mod structs;
mod type_definitions;
mod type_names;

pub use self::{assembly::*, contracts::*, enums::*, expressions::*, functions::*, import_directives::*, statements::*, storage::*, structs::*, type_definitions::*, type_names::*};

use crate::{errors::Error, sway};
use solang_parser::pt as solidity;
use std::{
    cell::RefCell,
    collections::HashMap,
    fmt::Display,
    path::{Path, PathBuf},
    rc::Rc
};

#[derive(Clone, Debug, PartialEq)]
pub struct TranslatedUsingDirective {
    pub library_name: String,
    pub for_type: Option<sway::TypeName>,
    pub functions: Vec<TranslatedFunction>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TranslatedEnum {
    pub type_definition: sway::TypeDefinition,
    pub variants_impl: sway::Impl,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TranslatedVariable {
    pub old_name: String,
    pub new_name: String,
    pub type_name: sway::TypeName,
    pub abi_type_name: Option<sway::TypeName>,
    pub is_storage: bool,
    pub is_configurable: bool,
    pub is_constant: bool,
    pub statement_index: Option<usize>,
    pub read_count: usize,
    pub mutation_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TranslatedFunction {
    pub old_name: String,
    pub new_name: String,
    pub parameters: sway::ParameterList,
    pub constructor_calls: Vec<sway::FunctionCall>,
    pub modifiers: Vec<sway::FunctionCall>,
    pub return_type: Option<sway::TypeName>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TranslatedModifier {
    pub old_name: String,
    pub new_name: String,
    pub parameters: sway::ParameterList,
    pub has_underscore: bool,
    pub pre_body: Option<sway::Block>,
    pub post_body: Option<sway::Block>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TranslationScope {
    pub parent: Option<Rc<RefCell<TranslationScope>>>,
    pub variables: Vec<Rc<RefCell<TranslatedVariable>>>,
    pub functions: Vec<Rc<RefCell<TranslatedFunction>>>,
}

impl TranslationScope {
    #[inline]
    pub fn generate_unique_variable_name(&self, name: &str) -> String {
        let mut result = name.to_string();

        while self.get_variable_from_new_name(result.as_str()).is_some() {
            result = format!("_{result}");
        }

        result
    }

    /// Attempts to get a reference to a translated variable using its old name
    pub fn get_variable_from_old_name(&self, old_name: &str) -> Option<Rc<RefCell<TranslatedVariable>>> {
        if let Some(variable) = self.variables.iter().rev().find(|v| v.borrow().old_name == old_name) {
            return Some(variable.clone());
        }

        if let Some(parent) = self.parent.as_ref() {
            if let Some(variable) = parent.borrow().get_variable_from_old_name(old_name) {
                return Some(variable);
            }
        }

        None
    }

    /// Attempts to get a reference to a translated variable using its new name
    pub fn get_variable_from_new_name(&self, new_name: &str) -> Option<Rc<RefCell<TranslatedVariable>>> {
        if let Some(variable) = self.variables.iter().rev().find(|v| v.borrow().new_name == new_name) {
            return Some(variable.clone());
        }

        if let Some(parent) = self.parent.as_ref() {
            if let Some(variable) = parent.borrow().get_variable_from_new_name(new_name) {
                return Some(variable);
            }
        }

        None
    }

    /// Attempts to find a translated variable using a custom function
    pub fn find_variable<F: Copy + FnMut(&&Rc<RefCell<TranslatedVariable>>) -> bool>(&self, f: F) -> Option<Rc<RefCell<TranslatedVariable>>> {
        if let Some(variable) = self.variables.iter().find(f) {
            return Some(variable.clone());
        }

        if let Some(parent) = self.parent.as_ref() {
            if let Some(variable) = parent.borrow().find_variable(f) {
                return Some(variable);
            }
        }

        None
    }

    #[inline]
    pub fn find_function_matching_types(
        &self,
        old_name: &str,
        parameters: &[sway::Expression],
        parameter_types: &[sway::TypeName],
    ) -> Option<Rc<RefCell<TranslatedFunction>>> {

        self.find_function(|f| {
            let f = f.borrow();

            // Ensure the function's old name matches the function call we're translating
            if f.old_name != old_name {
                return false;
            }

            // Ensure the supplied function call args match the function's parameters
            if parameters.len() != f.parameters.entries.len() {
                return false;
            }

            for (i, value_type_name) in parameter_types.iter().enumerate() {
                let Some(parameter_type_name) = f.parameters.entries[i].type_name.as_ref() else { continue };

                if !value_type_name.is_compatible_with(parameter_type_name) {
                    return false;
                }
            }

            true
        })
    }

    /// Atempts to find a translated function using a custom function
    pub fn find_function<F: Copy + FnMut(&&Rc<RefCell<TranslatedFunction>>) -> bool>(&self, f: F) -> Option<Rc<RefCell<TranslatedFunction>>> {
        if let Some(function) = self.functions.iter().find(f) {
            return Some(function.clone());
        }

        if let Some(parent) = self.parent.as_ref() {
            if let Some(function) = parent.borrow().find_function(f) {
                return Some(function);
            }
        }

        None
    }
}

#[derive(Clone, Debug, Default)]
pub struct TranslatedDefinition {
    pub path: PathBuf,
    pub toplevel_scope: Rc<RefCell<TranslationScope>>,
    pub kind: Option<solidity::ContractTy>,
    pub dependencies: Vec<String>,

    pub uses: Vec<sway::Use>,
    pub name: String,
    pub inherits: Vec<String>,
    pub using_directives: Vec<TranslatedUsingDirective>,
    pub type_definitions: Vec<sway::TypeDefinition>,
    pub structs: Vec<sway::Struct>,
    pub enums: Vec<TranslatedEnum>,
    pub events_enums: Vec<(sway::Enum, sway::Impl)>,
    pub errors_enums: Vec<(sway::Enum, sway::Impl)>,
    pub constants: Vec<sway::Constant>,
    pub abis: Vec<sway::Abi>,
    pub abi: Option<sway::Abi>,
    pub configurable: Option<sway::Configurable>,
    pub storage: Option<sway::Storage>,
    pub modifiers: Vec<TranslatedModifier>,
    pub functions: Vec<sway::Function>,
    pub impls: Vec<sway::Impl>,

    pub struct_names: Vec<String>,
    pub contract_names: Vec<String>,
    
    pub function_name_counts: HashMap<String, usize>,
    pub function_names: HashMap<String, String>,
    pub function_call_counts: HashMap<String, usize>,

    pub storage_fields_name_counts: HashMap<String, usize>,
    pub storage_fields_names: HashMap<String, String>,
}

impl Display for TranslatedDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut written = 0usize;

        for use_entry in self.uses.iter() {
            writeln!(f, "{}", sway::TabbedDisplayer(use_entry))?;
            written += 1;
        }

        for (i, x) in self.constants.iter().enumerate() {
            if i == 0 && written > 0 {
                writeln!(f)?;
            }

            writeln!(f, "{}", sway::TabbedDisplayer(x))?;
            written += 1;
        }

        for (i, x) in self.type_definitions.iter().enumerate() {
            if i == 0 && written > 0 {
                writeln!(f)?;
            }

            writeln!(f, "{}", sway::TabbedDisplayer(x))?;
            written += 1;
        }

        for (i, x) in self.enums.iter().enumerate() {
            if i == 0 && written > 0 {
                writeln!(f)?;
            } else if i > 0 {
                writeln!(f)?;
            }

            writeln!(f, "{}", sway::TabbedDisplayer(&x.type_definition))?;
            writeln!(f)?;
            writeln!(f, "{}", sway::TabbedDisplayer(&x.variants_impl))?;
            written += 1;
        }

        for (i, x) in self.structs.iter().enumerate() {
            if i == 0 && written > 0 {
                writeln!(f)?;
            } else if i > 0 {
                writeln!(f)?;
            }

            writeln!(f, "{}", sway::TabbedDisplayer(x))?;
            written += 1;
        }
        
        for (i, (events_enum, abi_encode_impl)) in self.events_enums.iter().enumerate() {
            if i == 0 && written > 0 {
                writeln!(f)?;
            } else if i > 0 {
                writeln!(f)?;
            }

            writeln!(f, "{}", sway::TabbedDisplayer(events_enum))?;
            writeln!(f)?;
            writeln!(f, "{}", sway::TabbedDisplayer(abi_encode_impl))?;
            written += 1;
        }

        for (i, (errors_enum, abi_encode_impl)) in self.errors_enums.iter().enumerate() {
            if i == 0 && written > 0 {
                writeln!(f)?;
            } else if i > 0 {
                writeln!(f)?;
            }

            writeln!(f, "{}", sway::TabbedDisplayer(errors_enum))?;
            writeln!(f)?;
            writeln!(f, "{}", sway::TabbedDisplayer(abi_encode_impl))?;
            written += 1;
        }
        
        for (i, x) in self.abis.iter().enumerate() {
            if i == 0 && written > 0 {
                writeln!(f)?;
            } else if i > 0 {
                writeln!(f)?;
            }

            writeln!(f, "{}", sway::TabbedDisplayer(x))?;
            written += 1;
        }
        
        if let Some(x) = self.abi.as_ref() {
            if written > 0 {
                writeln!(f)?;
            }

            writeln!(f, "{}", sway::TabbedDisplayer(x))?;
            written += 1;
        }
        
        if let Some(x) = self.storage.as_ref() {
            if written > 0 {
                writeln!(f)?;
            }

            writeln!(f, "{}", sway::TabbedDisplayer(x))?;
            written += 1;
        }
        
        if let Some(x) = self.configurable.as_ref() {
            if written > 0 {
                writeln!(f)?;
            }

            writeln!(f, "{}", sway::TabbedDisplayer(x))?;
            written += 1;
        }

        for (i, x) in self.functions.iter().enumerate() {
            if i == 0 && written > 0 {
                writeln!(f)?;
            } else if i > 0 {
                writeln!(f)?;
            }

            writeln!(f, "{}", sway::TabbedDisplayer(x))?;
            written += 1;
        }
        
        for (i, x) in self.impls.iter().enumerate() {
            if i == 0 && written > 0 {
                writeln!(f)?;
            } else if i > 0 {
                writeln!(f)?;
            }

            writeln!(f, "{}", sway::TabbedDisplayer(x))?;
            written += 1;
        }

        Ok(())
    }
}

impl Into<sway::Module> for TranslatedDefinition {
    fn into(self) -> sway::Module {
        let mut result = sway::Module {
            kind: match self.kind.as_ref().unwrap() {
                solidity::ContractTy::Abstract(_)
                | solidity::ContractTy::Contract(_)
                | solidity::ContractTy::Interface(_) => sway::ModuleKind::Contract,

                solidity::ContractTy::Library(_) => sway::ModuleKind::Library,
            },
            items: vec![],
        };

        for x in self.uses.iter() {
            result.items.push(sway::ModuleItem::Use(x.clone()));
        }

        for x in self.constants.iter() {
            result.items.push(sway::ModuleItem::Constant(x.clone()));
        }

        for x in self.type_definitions.iter() {
            result.items.push(sway::ModuleItem::TypeDefinition(x.clone()));
        }

        for x in self.enums.iter() {
            result.items.push(sway::ModuleItem::TypeDefinition(x.type_definition.clone()));
            result.items.push(sway::ModuleItem::Impl(x.variants_impl.clone()));
        }

        for x in self.structs.iter() {
            result.items.push(sway::ModuleItem::Struct(x.clone()));
        }
        
        for (events_enum, abi_encode_impl) in self.events_enums.iter() {
            result.items.push(sway::ModuleItem::Enum(events_enum.clone()));
            result.items.push(sway::ModuleItem::Impl(abi_encode_impl.clone()));
        }

        for (errors_enum, abi_encode_impl) in self.errors_enums.iter() {
            result.items.push(sway::ModuleItem::Enum(errors_enum.clone()));
            result.items.push(sway::ModuleItem::Impl(abi_encode_impl.clone()));
        }
        
        for x in self.abis.iter() {
            result.items.push(sway::ModuleItem::Abi(x.clone()));
        }
        
        if let Some(x) = self.abi.as_ref() {
            result.items.push(sway::ModuleItem::Abi(x.clone()));
        }
        
        if let Some(x) = self.storage.as_ref() {
            result.items.push(sway::ModuleItem::Storage(x.clone()));
        }
        
        if let Some(x) = self.configurable.as_ref() {
            result.items.push(sway::ModuleItem::Configurable(x.clone()));
        }

        for x in self.functions.iter() {
            if let Some(0) = self.function_call_counts.get(&x.name) {
                continue;
            }

            result.items.push(sway::ModuleItem::Function(x.clone()));
        }
        
        for x in self.impls.iter() {
            result.items.push(sway::ModuleItem::Impl(x.clone()));
        }

        result
    }
}

impl TranslatedDefinition {
    pub fn new<P: AsRef<Path>, S1: ToString, S2: ToString>(path: P, kind: solidity::ContractTy, name: S1, inherits: Vec<S2>) -> Self {
        Self {
            path: path.as_ref().into(),
            toplevel_scope: Rc::new(RefCell::new(TranslationScope::default())),
            kind: Some(kind),
            dependencies: vec![],

            uses: vec![],
            name: name.to_string(),
            inherits: inherits.iter().map(|i| i.to_string()).collect(),
            using_directives: vec![],
            type_definitions: vec![],
            enums: vec![],
            structs: vec![],
            events_enums: vec![],
            errors_enums: vec![],
            constants: vec![],
            abis: vec![],
            abi: None,
            configurable: None,
            storage: None,
            modifiers: vec![],
            functions: vec![],
            impls: vec![],

            struct_names: vec![],
            contract_names: vec![],

            function_name_counts: HashMap::new(),
            function_names: HashMap::new(),
            function_call_counts: HashMap::new(),

            storage_fields_name_counts: HashMap::new(),
            storage_fields_names: HashMap::new(),
        }
    }

    #[inline]
    pub fn ensure_dependency_declared(&mut self, dependency: &str) {
        let dependency = dependency.to_string();

        if !self.dependencies.contains(&dependency) {
            self.dependencies.push(dependency);
            self.dependencies.sort();
        }
    }

    #[inline]
    pub fn ensure_use_declared(&mut self, name: &str) {
        let mut tree: Option<sway::UseTree> = None;
        
        for part in name.split("::").collect::<Vec<_>>().into_iter().rev() {
            match part {
                "*" => tree = Some(sway::UseTree::Glob),
                
                _ => tree = Some(if let Some(use_tree) = tree.clone() {
                    sway::UseTree::Path {
                        prefix: part.into(),
                        suffix: Box::new(use_tree),
                    }
                } else {
                    sway::UseTree::Name {
                        name: part.into(),
                    }
                }),
            }
        }

        let tree = tree.unwrap();

        if !self.uses.iter().any(|u| u.tree == tree) {
            self.uses.push(sway::Use {
                is_public: false,
                tree,
            });
        }
    }

    #[inline]
    pub fn import_enum(&mut self, translated_enum: &TranslatedEnum) {
        let sway::TypeName::Identifier { name, generic_parameters: None } = &translated_enum.type_definition.name else {
            panic!("Expected Identifier type name, found {:#?}", translated_enum.type_definition.name);
        };
    
        for item in translated_enum.variants_impl.items.iter() {
            let sway::ImplItem::Constant(c) = item else { continue };
            
            self.toplevel_scope.borrow_mut().variables.push(Rc::new(RefCell::new(TranslatedVariable {
                old_name: String::new(), // TODO: is this ok?
                new_name: format!("{}::{}", name, c.name),
                type_name: translated_enum.type_definition.name.clone(),
                ..Default::default()
            })));
        }
        
        self.enums.push(translated_enum.clone());
    }
    
    /// Gets the abi for the translated definition. If it doesn't exist, it gets created.
    #[inline]
    pub fn get_abi(&mut self) -> &mut sway::Abi {
        if self.abi.is_none() {
            self.abi = Some(sway::Abi {
                name: self.name.clone(),
                inherits: vec![],
                functions: vec![],
            });
        }

        self.abi.as_mut().unwrap()
    }

    /// Gets the configurable block for the translated definition. If it doesn't exist, it gets created.
    #[inline]
    pub fn get_configurable(&mut self) -> &mut sway::Configurable {
        if self.configurable.is_none() {
            self.configurable = Some(sway::Configurable {
                fields: vec![],
            });
        }

        self.configurable.as_mut().unwrap()
    }

    /// Gets the storage block for the translated definition. If it doesn't exist, it gets created.
    #[inline]
    pub fn get_storage(&mut self) -> &mut sway::Storage {
        if self.storage.is_none() {
            self.storage = Some(sway::Storage {
                fields: vec![],
            });
        }

        self.storage.as_mut().unwrap()
    }

    #[inline]
    pub fn find_contract_impl(&self) -> Option<&sway::Impl> {
        self.impls.iter().find(|i| {
            let sway::TypeName::Identifier { name: type_name, .. } = &i.type_name else { return false };
            let Some(sway::TypeName::Identifier { name: for_type_name, .. }) = i.for_type_name.as_ref() else { return false };
            *type_name == self.name && for_type_name == "Contract"
        })
    }

    #[inline]
    pub fn find_contract_impl_mut(&mut self) -> Option<&mut sway::Impl> {
        self.impls.iter_mut().find(|i| {
            let sway::TypeName::Identifier { name: type_name, .. } = &i.type_name else { return false };
            let Some(sway::TypeName::Identifier { name: for_type_name, .. }) = i.for_type_name.as_ref() else { return false };
            *type_name == self.name && for_type_name == "Contract"
        })
    }

    /// Gets the translated definition's implementation for `Contract`. If it doesn't exist, it gets created.
    #[inline]
    pub fn get_contract_impl(&mut self) -> &mut sway::Impl {
        if self.find_contract_impl().is_none() {
            self.impls.push(sway::Impl {
                generic_parameters: None,
                type_name: sway::TypeName::Identifier {
                    name: self.name.clone(),
                    generic_parameters: None,
                },
                for_type_name: Some(sway::TypeName::Identifier {
                    name: "Contract".into(),
                    generic_parameters: None,
                }),
                items: vec![],
            });
        }

        self.find_contract_impl_mut().unwrap()
    }

    // Gets the base underlying type of the supplied type name
    pub fn get_underlying_type(&self, type_name: &sway::TypeName) -> sway::TypeName {
        // Check to see if the expression's type is a type definition and get the underlying type
        for type_definition in self.type_definitions.iter() {
            if &type_definition.name == type_name {
                return self.get_underlying_type(
                    type_definition.underlying_type.as_ref().unwrap(),
                );
            }
        }

        // If we didn't find a type definition, check to see if an enum exists and get its underlying type
        for translated_enum in self.enums.iter() {
            if &translated_enum.type_definition.name == type_name {
                return self.get_underlying_type(
                    translated_enum.type_definition.underlying_type.as_ref().unwrap(),
                );
            }
        }

        type_name.clone()
    }

    pub fn get_expression_type(
        &self,
        scope: Rc<RefCell<TranslationScope>>,
        expression: &sway::Expression,
    ) -> Result<sway::TypeName, Error> {
        match expression {
            sway::Expression::Literal(literal) => match literal {
                sway::Literal::Bool(_) => Ok(sway::TypeName::Identifier {
                    name: "bool".into(),
                    generic_parameters: None,
                }),
                sway::Literal::DecInt(_) => Ok(sway::TypeName::Identifier {
                    name: "u64".into(), // TODO: is this ok?
                    generic_parameters: None,
                }),
                sway::Literal::HexInt(_) => Ok(sway::TypeName::Identifier {
                    name: "u64".into(), // TODO: is this ok?
                    generic_parameters: None,
                }),
                sway::Literal::String(_) => Ok(sway::TypeName::StringSlice),
            }

            sway::Expression::Identifier(name) => {
                // HACK: Check if the identifier is a translated enum variant
                if name.contains("::") {
                    let parts = name.split("::").collect::<Vec<_>>();

                    if parts.len() == 2 {
                        let enum_name = parts[0];
                        let variant_name = parts[1];

                        if self.enums.iter().any(|e| {
                            let sway::TypeName::Identifier { name, generic_parameters: None } = &e.type_definition.name else { return false };
                            
                            if !e.variants_impl.items.iter().any(|i| {
                                let sway::ImplItem::Constant(variant) = i else { return false };
                                variant.name == variant_name
                            }) {
                                return false;
                            }

                            name == enum_name
                        }) {
                            return Ok(sway::TypeName::Identifier {
                                name: enum_name.into(),
                                generic_parameters: None,
                            });
                        }
                    }
                }

                let Some(variable) = scope.borrow().get_variable_from_new_name(name) else {
                    panic!("error: Variable not found in scope: \"{name}\"");
                };
        
                let variable = variable.borrow();

                // Variable should not be a storage field
                if variable.is_storage {
                    panic!("error: Variable not found in scope: \"{name}\"");
                }

                Ok(variable.type_name.clone())
            }

            sway::Expression::FunctionCall(_) | sway::Expression::FunctionCallBlock(_) => {
                let (function, parameters) = match expression {
                    sway::Expression::FunctionCall(f) => (&f.function, &f.parameters),
                    sway::Expression::FunctionCallBlock(f) => (&f.function, &f.parameters),
                    _ => unreachable!(),
                };

                match function {
                    sway::Expression::Identifier(name) => match name.as_str() {
                        "todo!" => Ok(sway::TypeName::Identifier {
                            name: "todo!".into(),
                            generic_parameters: None,
                        }),
    
                        "abi" => {
                            if parameters.len() != 2 {
                                panic!("Malformed abi cast, expected 2 parameters, found {}", parameters.len());
                            }
    
                            let sway::Expression::Identifier(definition_name) = &parameters[0] else {
                                panic!("Malformed abi cast, expected identifier, found {:#?}", parameters[0]);
                            };
    
                            Ok(sway::TypeName::Identifier {
                                name: definition_name.clone(),
                                generic_parameters: None,
                            })
                        }
    
                        "b256::from" => Ok(sway::TypeName::Identifier {
                            name: "b256".into(),
                            generic_parameters: None,
                        }),
    
                        "Bytes::new" | "Bytes::from" => Ok(sway::TypeName::Identifier {
                            name: "Bytes".into(),
                            generic_parameters: None,
                        }),
    
                        "Identity::Address" | "Identity::ContractId" | "Identity::from" => Ok(sway::TypeName::Identifier {
                            name: "Identity".into(),
                            generic_parameters: None,
                        }),
    
                        "msg_sender" => Ok(sway::TypeName::Identifier {
                            name: "Option".into(),
                            generic_parameters: Some(sway::GenericParameterList {
                                entries: vec![
                                    sway::GenericParameter {
                                        type_name: sway::TypeName::Identifier {
                                            name: "Identity".into(),
                                            generic_parameters: None,
                                        },
                                        implements: None,
                                    },
                                ],
                            }),
                        }),
    
                        "std::block::height" => Ok(sway::TypeName::Identifier {
                            name: "u32".into(),
                            generic_parameters: None,
                        }),
                        
                        "std::block::timestamp" => Ok(sway::TypeName::Identifier {
                            name: "u64".into(),
                            generic_parameters: None,
                        }),
                        
                        "std::context::this_balance" => Ok(sway::TypeName::Identifier {
                            name: "u64".into(),
                            generic_parameters: None,
                        }),
    
                        "std::hash::keccak256" => Ok(sway::TypeName::Identifier {
                            name: "b256".into(),
                            generic_parameters: None,
                        }),
    
                        "u64::try_from" => Ok(sway::TypeName::Identifier {
                            name: "Option".into(),
                            generic_parameters: Some(sway::GenericParameterList {
                                entries: vec![
                                    sway::GenericParameter {
                                        type_name: sway::TypeName::Identifier {
                                            name: "u64".into(),
                                            generic_parameters: None,
                                        },
                                        implements: None,
                                    },
                                ],
                            }),
                        }),
    
                        new_name => {
                            let parameter_types = parameters.iter()
                                .map(|p| self.get_expression_type(scope.clone(), p))
                                .collect::<Result<Vec<_>, _>>()?;
                            
                            // Ensure the function exists in scope
                            let Some(function) = scope.borrow().find_function(|f| {
                                let f = f.borrow();
    
                                // Ensure the function's new name matches the function call we're translating
                                if f.new_name != new_name {
                                    return false;
                                }
                                
                                // Ensure the supplied function call args match the function's parameters
                                if parameters.len() != f.parameters.entries.len() {
                                    return false;
                                }
    
                                for (i, value_type_name) in parameter_types.iter().enumerate() {
                                    let Some(parameter_type_name) = f.parameters.entries[i].type_name.as_ref() else { continue };
    
                                    if !value_type_name.is_compatible_with(parameter_type_name) {
                                        return false;
                                    }
                                }
    
                                true
                            }) else {
                                panic!("Failed to find function `{new_name}` in scope");
                            };
    
                            let function = function.borrow();
    
                            if let Some(return_type) = function.return_type.as_ref() {
                                Ok(return_type.clone())
                            } else {
                                Ok(sway::TypeName::Tuple { type_names: vec![] })
                            }
                        }
                    }
    
                    sway::Expression::MemberAccess(member_access) => match self.get_expression_type(scope.clone(), &member_access.expression)? {
                        sway::TypeName::Undefined => panic!("Undefined type name"),
    
                        sway::TypeName::Identifier { name, generic_parameters } => match (name.as_str(), generic_parameters.as_ref()) {
                            ("b256", None) => match member_access.member.as_str() {
                                "as_u256" => Ok(sway::TypeName::Identifier {
                                    name: "u256".into(),
                                    generic_parameters: None,
                                }),
    
                                "to_be_bytes" => Ok(sway::TypeName::Identifier {
                                    name: "Bytes".into(),
                                    generic_parameters: None,
                                }),
    
                                "to_le_bytes" => Ok(sway::TypeName::Identifier {
                                    name: "Bytes".into(),
                                    generic_parameters: None,
                                }),
    
                                _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                            }
    
                            ("u8", None) => match member_access.member.as_str() {
                                "as_u16" => Ok(sway::TypeName::Identifier {
                                    name: "u16".into(),
                                    generic_parameters: None,
                                }),
    
                                "as_u32" => Ok(sway::TypeName::Identifier {
                                    name: "u32".into(),
                                    generic_parameters: None,
                                }),
    
                                "as_u64" => Ok(sway::TypeName::Identifier {
                                    name: "u64".into(),
                                    generic_parameters: None,
                                }),
    
                                "as_u256" => Ok(sway::TypeName::Identifier {
                                    name: "u256".into(),
                                    generic_parameters: None,
                                }),
    
                                "pow" => Ok(sway::TypeName::Identifier {
                                    name: "u8".into(),
                                    generic_parameters: None,
                                }),
    
                                _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                            }
    
                            ("u16", None) => match member_access.member.as_str() {
                                "as_u32" => Ok(sway::TypeName::Identifier {
                                    name: "u32".into(),
                                    generic_parameters: None,
                                }),
    
                                "as_u64" => Ok(sway::TypeName::Identifier {
                                    name: "u64".into(),
                                    generic_parameters: None,
                                }),
    
                                "as_u256" => Ok(sway::TypeName::Identifier {
                                    name: "u256".into(),
                                    generic_parameters: None,
                                }),
    
                                "pow" => Ok(sway::TypeName::Identifier {
                                    name: "u16".into(),
                                    generic_parameters: None,
                                }),
    
                                "to_be_bytes" => Ok(sway::TypeName::Identifier {
                                    name: "Bytes".into(),
                                    generic_parameters: None,
                                }),
    
                                "to_le_bytes" => Ok(sway::TypeName::Identifier {
                                    name: "Bytes".into(),
                                    generic_parameters: None,
                                }),
    
                                _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                            }
    
                            ("u32", None) => match member_access.member.as_str() {
                                "as_u64" => Ok(sway::TypeName::Identifier {
                                    name: "u64".into(),
                                    generic_parameters: None,
                                }),
    
                                "as_u256" => Ok(sway::TypeName::Identifier {
                                    name: "u256".into(),
                                    generic_parameters: None,
                                }),
    
                                "pow" => Ok(sway::TypeName::Identifier {
                                    name: "u32".into(),
                                    generic_parameters: None,
                                }),
    
                                "to_be_bytes" => Ok(sway::TypeName::Identifier {
                                    name: "Bytes".into(),
                                    generic_parameters: None,
                                }),
    
                                "to_le_bytes" => Ok(sway::TypeName::Identifier {
                                    name: "Bytes".into(),
                                    generic_parameters: None,
                                }),
    
                                _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                            }
    
                            ("u64", None) => match member_access.member.as_str() {
                                "as_u256" => Ok(sway::TypeName::Identifier {
                                    name: "u256".into(),
                                    generic_parameters: None,
                                }),
    
                                "pow" => Ok(sway::TypeName::Identifier {
                                    name: "u64".into(),
                                    generic_parameters: None,
                                }),
    
                                "to_be_bytes" => Ok(sway::TypeName::Identifier {
                                    name: "Bytes".into(),
                                    generic_parameters: None,
                                }),
    
                                "to_le_bytes" => Ok(sway::TypeName::Identifier {
                                    name: "Bytes".into(),
                                    generic_parameters: None,
                                }),
    
                                _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                            }
    
                            ("u256", None) => match member_access.member.as_str() {
                                "as_b256" => Ok(sway::TypeName::Identifier {
                                    name: "b256".into(),
                                    generic_parameters: None,
                                }),
    
                                "pow" => Ok(sway::TypeName::Identifier {
                                    name: "u256".into(),
                                    generic_parameters: None,
                                }),
    
                                "to_be_bytes" => Ok(sway::TypeName::Identifier {
                                    name: "Bytes".into(),
                                    generic_parameters: None,
                                }),
    
                                "to_le_bytes" => Ok(sway::TypeName::Identifier {
                                    name: "Bytes".into(),
                                    generic_parameters: None,
                                }),
    
                                _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                            }
    
                            ("Bytes", None) => match member_access.member.as_str() {
                                "len" => Ok(sway::TypeName::Identifier {
                                    name: "u64".into(),
                                    generic_parameters: None,
                                }),
    
                                "split_at" => Ok(sway::TypeName::Tuple {
                                    type_names: vec![
                                        sway::TypeName::Identifier {
                                            name: "Bytes".into(),
                                            generic_parameters: None,
                                        },
                                        sway::TypeName::Identifier {
                                            name: "Bytes".into(),
                                            generic_parameters: None,
                                        },
                                    ],
                                }),
                                "as_raw_slice" => Ok(sway::TypeName::Identifier {
                                    name: "RawSlice".into(),
                                    generic_parameters: None,
                                }),
    
                                _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                            }
    
                            ("Identity", None) => match member_access.member.as_str() {
                                "as_address" => Ok(sway::TypeName::Identifier {
                                    name: "Option".into(),
                                    generic_parameters: Some(sway::GenericParameterList {
                                        entries: vec![
                                            sway::GenericParameter {
                                                type_name: sway::TypeName::Identifier {
                                                    name: "Address".into(),
                                                    generic_parameters: None,
                                                },
                                                implements: None,
                                            },
                                        ],
                                    }),
                                }),
    
                                "as_contract_id" => Ok(sway::TypeName::Identifier {
                                    name: "Option".into(),
                                    generic_parameters: Some(sway::GenericParameterList {
                                        entries: vec![
                                            sway::GenericParameter {
                                                type_name: sway::TypeName::Identifier {
                                                    name: "ContractId".into(),
                                                    generic_parameters: None,
                                                },
                                                implements: None,
                                            },
                                        ],
                                    }),
                                }),
    
                                "is_address" => Ok(sway::TypeName::Identifier {
                                    name: "bool".into(),
                                    generic_parameters: None,
                                }),
    
                                "is_contract_id" => Ok(sway::TypeName::Identifier {
                                    name: "bool".into(),
                                    generic_parameters: None,
                                }),
    
                                _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                            }
    
                            ("Option", Some(generic_parameters)) if generic_parameters.entries.len() == 1 => match member_access.member.as_str() {
                                "unwrap" => Ok(generic_parameters.entries[0].type_name.clone()),
                                
                                _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                            }
                            
                            ("Result", Some(generic_parameters)) if generic_parameters.entries.len() == 2 => match member_access.member.as_str() {
                                "unwrap" => Ok(generic_parameters.entries[0].type_name.clone()),
                                
                                _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                            }
                            
                            ("StorageKey", Some(generic_parameters)) if generic_parameters.entries.len() == 1 => match member_access.member.as_str() {
                                "clear" => Ok(sway::TypeName::Identifier {
                                    name: "bool".into(),
                                    generic_parameters: None,
                                }),
    
                                "read" => Ok(generic_parameters.entries[0].type_name.clone()),
    
                                "try_read" => Ok(sway::TypeName::Identifier {
                                    name: "Option".into(),
                                    generic_parameters: Some(sway::GenericParameterList {
                                        entries: vec![
                                            sway::GenericParameter {
                                                type_name: generic_parameters.entries[0].type_name.clone(),
                                                implements: None,
                                            },
                                        ],
                                    }),
                                }),
    
                                "write" => Ok(sway::TypeName::Tuple { type_names: vec![] }),
    
                                _ => match &generic_parameters.entries[0].type_name {
                                    sway::TypeName::Identifier { name, generic_parameters } => match (name.as_str(), generic_parameters.as_ref()) {
                                        ("StorageBytes", None) => match member_access.member.as_str() {
                                            "clear" => Ok(sway::TypeName::Identifier {
                                                name: "bool".into(),
                                                generic_parameters: None,
                                            }),
    
                                            "len" => Ok(sway::TypeName::Identifier {
                                                name: "u64".into(),
                                                generic_parameters: None,
                                            }),
                            
                                            "read_slice" => Ok(sway::TypeName::Identifier {
                                                name: "Option".into(),
                                                generic_parameters: Some(sway::GenericParameterList {
                                                    entries: vec![
                                                        sway::GenericParameter {
                                                            type_name: sway::TypeName::Identifier {
                                                                name: "Bytes".into(),
                                                                generic_parameters: None,
                                                            },
                                                            implements: None,
                                                        },
                                                    ],
                                                }),
                                            }),
    
                                            "write_slice" => Ok(sway::TypeName::Tuple { type_names: vec![] }),
    
                                            _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                                        }
    
                                        ("StorageMap", Some(generic_parameters)) if generic_parameters.entries.len() == 2 => match member_access.member.as_str() {
                                            "get" => Ok(sway::TypeName::Identifier {
                                                name: "StorageKey".into(),
                                                generic_parameters: Some(sway::GenericParameterList {
                                                    entries: vec![
                                                        sway::GenericParameter {
                                                            type_name: generic_parameters.entries[1].type_name.clone(),
                                                            implements: None,
                                                        },
                                                    ],
                                                }),
                                            }),
    
                                            "insert" => Ok(sway::TypeName::Tuple { type_names: vec![] }),
    
                                            "remove" => Ok(sway::TypeName::Identifier {
                                                name: "bool".into(),
                                                generic_parameters: None,
                                            }),
    
                                            "try_insert" => Ok(sway::TypeName::Identifier {
                                                name: "Result".into(),
                                                generic_parameters: Some(sway::GenericParameterList {
                                                    entries: vec![
                                                        sway::GenericParameter {
                                                            type_name: generic_parameters.entries[0].type_name.clone(),
                                                            implements: None,
                                                        },
                                                        sway::GenericParameter {
                                                            type_name: sway::TypeName::Identifier {
                                                                name: "StorageMapError".into(),
                                                                generic_parameters: Some(sway::GenericParameterList {
                                                                    entries: vec![
                                                                        sway::GenericParameter {
                                                                            type_name: generic_parameters.entries[0].type_name.clone(),
                                                                            implements: None,
                                                                        },
                                                                    ],
                                                                }),
                                                            },
                                                            implements: None,
                                                        },
                                                    ],
                                                }),
                                            }),
    
                                            _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                                        }
    
                                        ("StorageString", None) => match member_access.member.as_str() {
                                            "clear" => Ok(sway::TypeName::Identifier {
                                                name: "bool".into(),
                                                generic_parameters: None,
                                            }),
    
                                            "len" => Ok(sway::TypeName::Identifier {
                                                name: "u64".into(),
                                                generic_parameters: None,
                                            }),
                            
                                            "read_slice" => Ok(sway::TypeName::Identifier {
                                                name: "Option".into(),
                                                generic_parameters: Some(sway::GenericParameterList {
                                                    entries: vec![
                                                        sway::GenericParameter {
                                                            type_name: sway::TypeName::Identifier {
                                                                name: "String".into(),
                                                                generic_parameters: None,
                                                            },
                                                            implements: None,
                                                        },
                                                    ],
                                                }),
                                            }),
    
                                            "write_slice" => Ok(sway::TypeName::Tuple { type_names: vec![] }),
    
                                            _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                                        }
    
                                        ("StorageVec", Some(generic_parameters)) if generic_parameters.entries.len() == 1 => match member_access.member.as_str() {
                                            "fill" => Ok(sway::TypeName::Tuple { type_names: vec![] }),
    
                                            "first" => Ok(sway::TypeName::Identifier {
                                                name: "Option".into(),
                                                generic_parameters: Some(sway::GenericParameterList {
                                                    entries: vec![
                                                        sway::GenericParameter {
                                                            type_name: sway::TypeName::Identifier {
                                                                name: "StorageKey".into(),
                                                                generic_parameters: Some(sway::GenericParameterList {
                                                                    entries: vec![
                                                                        sway::GenericParameter {
                                                                            type_name: generic_parameters.entries[0].type_name.clone(),
                                                                            implements: None,
                                                                        },
                                                                    ],
                                                                }),
                                                            },
                                                            implements: None,
                                                        },
                                                    ],
                                                }),
                                            }),
    
                                            "get" => Ok(sway::TypeName::Identifier {
                                                name: "Option".into(),
                                                generic_parameters: Some(sway::GenericParameterList {
                                                    entries: vec![
                                                        sway::GenericParameter {
                                                            type_name: sway::TypeName::Identifier {
                                                                name: "StorageKey".into(),
                                                                generic_parameters: Some(sway::GenericParameterList {
                                                                    entries: vec![
                                                                        sway::GenericParameter {
                                                                            type_name: generic_parameters.entries[0].type_name.clone(),
                                                                            implements: None,
                                                                        },
                                                                    ],
                                                                }),
                                                            },
                                                            implements: None,
                                                        },
                                                    ],
                                                }),
                                            }),
    
                                            "insert" => Ok(sway::TypeName::Tuple { type_names: vec![] }),
    
                                            "is_empty" => Ok(sway::TypeName::Identifier {
                                                name: "bool".into(),
                                                generic_parameters: None,
                                            }),
    
                                            "last" => Ok(sway::TypeName::Identifier {
                                                name: "Option".into(),
                                                generic_parameters: Some(sway::GenericParameterList {
                                                    entries: vec![
                                                        sway::GenericParameter {
                                                            type_name: sway::TypeName::Identifier {
                                                                name: "StorageKey".into(),
                                                                generic_parameters: Some(sway::GenericParameterList {
                                                                    entries: vec![
                                                                        sway::GenericParameter {
                                                                            type_name: generic_parameters.entries[0].type_name.clone(),
                                                                            implements: None,
                                                                        },
                                                                    ],
                                                                }),
                                                            },
                                                            implements: None,
                                                        },
                                                    ],
                                                }),
                                            }),
    
                                            "len" => Ok(sway::TypeName::Identifier {
                                                name: "u64".into(),
                                                generic_parameters: None,
                                            }),
    
                                            "load_vec" => Ok(sway::TypeName::Identifier {
                                                name: "Vec".into(),
                                                generic_parameters: Some(sway::GenericParameterList {
                                                    entries: vec![
                                                        sway::GenericParameter {
                                                            type_name: generic_parameters.entries[0].type_name.clone(),
                                                            implements: None,
                                                        },
                                                    ],
                                                }),
                                            }),
    
                                            "pop" => Ok(sway::TypeName::Identifier {
                                                name: "Option".into(),
                                                generic_parameters: Some(sway::GenericParameterList {
                                                    entries: vec![
                                                        sway::GenericParameter {
                                                            type_name: generic_parameters.entries[0].type_name.clone(),
                                                            implements: None,
                                                        },
                                                    ],
                                                }),
                                            }),
    
                                            "push" => Ok(sway::TypeName::Tuple { type_names: vec![] }),
    
                                            "remove" => Ok(generic_parameters.entries[0].type_name.clone()),
    
                                            "resize" => Ok(sway::TypeName::Tuple { type_names: vec![] }),
    
                                            "reverse" => Ok(sway::TypeName::Tuple { type_names: vec![] }),
    
                                            "set" => Ok(sway::TypeName::Tuple { type_names: vec![] }),
    
                                            "store_vec" => Ok(sway::TypeName::Tuple { type_names: vec![] }),
    
                                            "swap_remove" => Ok(generic_parameters.entries[0].type_name.clone()),
    
                                            "swap" => Ok(sway::TypeName::Tuple { type_names: vec![] }),
    
                                            _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                                        }
    
                                        _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                                    }
    
                                    _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                                }
                            }
    
                            ("String", None) => match member_access.member.as_str() {
                                "len" => Ok(sway::TypeName::Identifier {
                                    name: "u64".into(),
                                    generic_parameters: None,
                                }),
    
                                _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                            }
    
                            ("Vec", Some(generic_parameters)) if generic_parameters.entries.len() == 1 => match member_access.member.as_str() {
                                "get" => Ok(sway::TypeName::Identifier {
                                    name: "Option".into(),
                                    generic_parameters: Some(sway::GenericParameterList {
                                        entries: vec![
                                            generic_parameters.entries.first().unwrap().clone(),
                                        ],
                                    }),
                                }),
    
                                "len" => Ok(sway::TypeName::Identifier {
                                    name: "u64".into(),
                                    generic_parameters: None,
                                }),
    
                                _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                            }
    
                            (name, None) => {
                                if let Some(abi) = self.abi.as_ref() {
                                    if abi.name == name {
                                        if let Some(function_definition) = abi.functions.iter().find(|f| f.name == member_access.member) {
                                            return Ok(function_definition.return_type.clone().unwrap_or_else(|| sway::TypeName::Tuple { type_names: vec![] }))
                                        }
                                    }
                                }
    
                                for abi in self.abis.iter() {
                                    if abi.name == name {
                                        if let Some(function_definition) = abi.functions.iter().find(|f| f.name == member_access.member) {
                                            return Ok(function_definition.return_type.clone().unwrap_or_else(|| sway::TypeName::Tuple { type_names: vec![] }))
                                        }
                                    }
                                }
    
                                todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression))
                            }
    
                            _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                        }
    
                        sway::TypeName::StringSlice => match member_access.member.as_str() {
                            "len" => Ok(sway::TypeName::Identifier {
                                name: "u64".into(),
                                generic_parameters: None,
                            }),
    
                            _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                        }
    
                        _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                    }
    
                    _ => todo!("get type of function call expression: {} - {expression:#?}", sway::TabbedDisplayer(expression)),
                }
            }

            sway::Expression::Block(block) => {
                let Some(expression) = block.final_expr.as_ref() else {
                    return Ok(sway::TypeName::Tuple { type_names: vec![] });
                };

                let inner_scope = Rc::new(RefCell::new(TranslationScope {
                    parent: Some(scope.clone()),
                    ..Default::default()
                }));

                for statement in block.statements.iter() {
                    let sway::Statement::Let(sway::Let {
                        pattern,
                        type_name,
                        value,
                    }) = statement else { continue };

                    let type_name = match type_name.as_ref() {
                        Some(type_name) => type_name.clone(),
                        None => self.get_expression_type(inner_scope.clone(), value)?,
                    };

                    let add_variable = |id: &sway::LetIdentifier, type_name: &sway::TypeName| {
                        inner_scope.borrow_mut().variables.push(Rc::new(RefCell::new(TranslatedVariable {
                            old_name: String::new(),
                            new_name: id.name.clone(),
                            type_name: type_name.clone(),
                            ..Default::default()
                        })));
                    };

                    match pattern {
                        sway::LetPattern::Identifier(id) => add_variable(id, &type_name),
                        sway::LetPattern::Tuple(ids) => {
                            let sway::TypeName::Tuple { type_names } = &type_name else {
                                panic!("Expected tuple type, found {type_name}");
                            };

                            for (id, type_name) in ids.iter().zip(type_names.iter()) {
                                add_variable(id, type_name);
                            }
                        }
                    }
                }

                self.get_expression_type(inner_scope, expression)
            }

            sway::Expression::Return(value) => {
                if let Some(value) = value.as_ref() {
                    self.get_expression_type(scope.clone(), value)
                } else {
                    Ok(sway::TypeName::Tuple { type_names: vec![] })
                }
            }

            sway::Expression::Array(array) => Ok(sway::TypeName::Array {
                type_name: Box::new(
                    if let Some(expression) = array.elements.first() {
                        self.get_expression_type(scope.clone(), expression)?
                    } else {
                        sway::TypeName::Tuple { type_names: vec![] }
                    }
                ),
                length: array.elements.len(),
            }),

            sway::Expression::ArrayAccess(array_access) => {
                let element_type_name = self.get_expression_type(scope.clone(), &array_access.expression)?;
                
                let type_name = match &element_type_name {
                    sway::TypeName::Identifier {
                        name,
                        generic_parameters: Some(generic_parameters),
                    } if name == "Vec" => {
                        &generic_parameters.entries.first().unwrap().type_name
                    }

                    sway::TypeName::Array { type_name, .. } => type_name.as_ref(),

                    _ => todo!("array access for type {element_type_name}"),
                };

                Ok(type_name.clone())
            }

            sway::Expression::MemberAccess(member_access) => match &member_access.expression {
                sway::Expression::Identifier(name) => match name.as_str() {
                    "storage" => {
                        let Some(variable) = scope.borrow().find_variable(|v| v.borrow().is_storage && v.borrow().new_name == member_access.member) else {
                            panic!("Failed to find storage variable in scope: `{}`", member_access.member);
                        };

                        let variable = variable.borrow();

                        Ok(sway::TypeName::Identifier {
                            name: "StorageKey".into(),
                            generic_parameters: Some(sway::GenericParameterList {
                                entries: vec![
                                    sway::GenericParameter {
                                        type_name: variable.type_name.clone(),
                                        implements: None,
                                    },
                                ],
                            }),
                        })
                    }

                    _ => {
                        let container_type = self.get_expression_type(scope.clone(), &member_access.expression)?;

                        match &container_type {
                            sway::TypeName::Identifier { name, generic_parameters } => match (name.as_str(), generic_parameters.as_ref()) {
                                ("I8", None) => match member_access.member.as_str() {
                                    "underlying" => Ok(sway::TypeName::Identifier {
                                        name: "u8".into(),
                                        generic_parameters: None,
                                    }),

                                    _ => todo!("get type of {container_type} member access expression: {expression:#?}"),
                                }

                                ("I16", None) => match member_access.member.as_str() {
                                    "underlying" => Ok(sway::TypeName::Identifier {
                                        name: "u16".into(),
                                        generic_parameters: None,
                                    }),

                                    _ => todo!("get type of {container_type} member access expression: {expression:#?}"),
                                }

                                ("I32", None) => match member_access.member.as_str() {
                                    "underlying" => Ok(sway::TypeName::Identifier {
                                        name: "u32".into(),
                                        generic_parameters: None,
                                    }),

                                    _ => todo!("get type of {container_type} member access expression: {expression:#?}"),
                                }

                                ("I64", None) => match member_access.member.as_str() {
                                    "underlying" => Ok(sway::TypeName::Identifier {
                                        name: "u64".into(),
                                        generic_parameters: None,
                                    }),

                                    _ => todo!("get type of {container_type} member access expression: {expression:#?}"),
                                }

                                ("I128", None) => match member_access.member.as_str() {
                                    "underlying" => Ok(sway::TypeName::Identifier {
                                        name: "u128".into(),
                                        generic_parameters: None,
                                    }),

                                    _ => todo!("get type of {container_type} member access expression: {expression:#?}"),
                                }

                                ("I256", None) => match member_access.member.as_str() {
                                    "underlying" => Ok(sway::TypeName::Identifier {
                                        name: "u256".into(),
                                        generic_parameters: None,
                                    }),

                                    _ => todo!("get type of {container_type} member access expression: {expression:#?}"),
                                }

                                _ => {
                                    // Check if container is a struct
                                    if let Some(struct_definition) = self.structs.iter().find(|s| s.name == *name) {
                                        if let Some(field) = struct_definition.fields.iter().find(|f| f.name == member_access.member) {
                                            return Ok(field.type_name.clone());
                                        }
                                    }

                                    todo!("get type of {container_type} member access expression: {expression:#?}")
                                }
                            }
                            
                            _ => todo!("get type of {container_type} member access expression: {expression:#?}"),
                        }
                    }
                }

                _ => todo!("get type of member access expression: {expression:#?}"),
            }
            
            sway::Expression::Tuple(tuple) => Ok(sway::TypeName::Tuple {
                type_names: tuple.iter().map(|x| self.get_expression_type(scope.clone(), x)).collect::<Result<Vec<_>, _>>()?,
            }),
            
            sway::Expression::If(if_expr) => {
                if let Some(expression) = if_expr.then_body.final_expr.as_ref() {
                    self.get_expression_type(scope.clone(), expression)
                } else {
                    Ok(sway::TypeName::Tuple { type_names: vec![] })
                }
            }

            sway::Expression::Match(match_expr) => {
                if let Some(branch) = match_expr.branches.first() {
                    self.get_expression_type(scope.clone(), &branch.value)
                } else {
                    Ok(sway::TypeName::Tuple { type_names: vec![] })
                }
            }
            
            sway::Expression::While(_) => Ok(sway::TypeName::Tuple { type_names: vec![] }),
            sway::Expression::UnaryExpression(unary_expression) => self.get_expression_type(scope.clone(), &unary_expression.expression),
            sway::Expression::BinaryExpression(binary_expression) => self.get_expression_type(scope.clone(), &binary_expression.lhs),
            sway::Expression::Constructor(constructor) => Ok(constructor.type_name.clone()),
            sway::Expression::Continue => Ok(sway::TypeName::Tuple { type_names: vec![] }),
            sway::Expression::Break => Ok(sway::TypeName::Tuple { type_names: vec![] }),
            
            sway::Expression::AsmBlock(_) => todo!("get type of asm block: {expression:#?}"),
            
            sway::Expression::Commented(_, x) => self.get_expression_type(scope.clone(), x),
        }
    }
}
