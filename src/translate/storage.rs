use super::{
    create_value_expression, translate_expression, translate_type_name, TranslatedDefinition,
    TranslatedVariable, TranslationScope,
};
use crate::{project::Project, sway, Error};
use convert_case::Case;
use solang_parser::pt as solidity;
use std::{cell::RefCell, rc::Rc};

#[inline]
pub fn translate_storage_name(
    _project: &mut Project,
    translated_definition: &mut TranslatedDefinition,
    name: &str,
) -> String {
    if !translated_definition.storage_fields_names.contains_key(name) {
        let mut new_name = crate::translate_naming_convention(name, Case::Snake);

        let count = translated_definition.storage_fields_name_counts.entry(new_name.clone()).or_insert(0);
        *count += 1;

        if *count > 1 {
            new_name = format!("{new_name}_{}", *count);
        }

        translated_definition.storage_fields_names.insert(name.into(), new_name);
    }

    translated_definition.storage_fields_names.get(name).unwrap().clone()
}

#[inline]
pub fn translate_state_variable(
    project: &mut Project,
    translated_definition: &mut TranslatedDefinition,
    variable_definition: &solidity::VariableDefinition,
) -> Result<(), Error> {
    // Collect information about the variable from its attributes
    let is_public = variable_definition.attrs.iter().any(|x| matches!(x, solidity::VariableAttribute::Visibility(solidity::Visibility::External(_) | solidity::Visibility::Public(_))));
    let is_constant = variable_definition.attrs.iter().any(|x| matches!(x, solidity::VariableAttribute::Constant(_)));
    let is_immutable = variable_definition.attrs.iter().any(|x| matches!(x, solidity::VariableAttribute::Immutable(_)));

    // If the state variable is not constant or immutable, it is a storage field
    let is_storage = !is_constant && !is_immutable;

    // If the state variable is immutable and not a constant, it is a configurable field
    let is_configurable = is_immutable && !is_constant;

    // Translate the variable's naming convention
    let old_name = variable_definition.name.as_ref().unwrap().name.clone();
    let new_name = if is_constant || is_immutable {
        crate::translate_naming_convention(old_name.as_str(), Case::ScreamingSnake)
    } else {
        translate_storage_name(project, translated_definition, old_name.as_str())
    };

    // Translate the variable's type name
    let mut variable_type_name = translate_type_name(project, translated_definition, &variable_definition.ty, is_storage);
    let mut abi_type_name = None;

    // Check if the variable's type is an ABI
    if let sway::TypeName::Identifier { name, generic_parameters: None } = &variable_type_name {
        if project.find_definition_with_abi(name.as_str()).is_some() {
            abi_type_name = Some(variable_type_name.clone());

            variable_type_name = sway::TypeName::Identifier {
                name: "Identity".into(),
                generic_parameters: None,
            };
        }
    }

    // Handle constant variable definitions
    if is_constant {
        let value = if let Some(x) = variable_definition.initializer.as_ref() {
            let scope = Rc::new(RefCell::new(TranslationScope {
                parent: Some(translated_definition.toplevel_scope.clone()),
                ..Default::default()
            }));

            let x = translate_expression(project, translated_definition, scope.clone(), x)?;
            Some(create_value_expression(translated_definition, scope.clone(), &variable_type_name, Some(&x)))
        } else {
            None
        };

        translated_definition.constants.push(sway::Constant {
            is_public,
            name: new_name.clone(),
            type_name: variable_type_name.clone(),
            value,
        });
    }
    // Handle immutable variable definitions
    else if is_immutable {
        //
        // TODO: we need to check if the value is supplied to the constructor and remove it from there
        //

        let value = if let Some(x) = variable_definition.initializer.as_ref() {
            let scope = Rc::new(RefCell::new(TranslationScope {
                parent: Some(translated_definition.toplevel_scope.clone()),
                ..Default::default()
            }));

            let x = translate_expression(project, translated_definition, scope.clone(), x)?;
            create_value_expression(translated_definition, scope.clone(), &variable_type_name, Some(&x))
        } else {
            let scope = Rc::new(RefCell::new(TranslationScope {
                parent: Some(translated_definition.toplevel_scope.clone()),
                ..Default::default()
            }));

            create_value_expression(translated_definition, scope.clone(), &variable_type_name, None)
        };

        translated_definition.get_configurable().fields.push(sway::ConfigurableField {
            name: new_name.clone(), 
            type_name: variable_type_name.clone(),
            value,
        });
    }
    // Handle regular state variable definitions
    else {
        let value = if let Some(x) = variable_definition.initializer.as_ref() {
            let scope = Rc::new(RefCell::new(TranslationScope {
                parent: Some(translated_definition.toplevel_scope.clone()),
                ..Default::default()
            }));

            let x = translate_expression(project, translated_definition, scope.clone(), x)?;
            create_value_expression(translated_definition, scope.clone(), &variable_type_name, Some(&x))
        } else {
            let scope = Rc::new(RefCell::new(TranslationScope {
                parent: Some(translated_definition.toplevel_scope.clone()),
                ..Default::default()
            }));

            create_value_expression(translated_definition, scope.clone(), &variable_type_name, None)
        };

        translated_definition.get_storage().fields.push(sway::StorageField {
            name: new_name.clone(),
            type_name: variable_type_name.clone(),
            value,
        });
    }
    
    // Add the storage variable for function scopes
    translated_definition.toplevel_scope.borrow_mut().variables.push(Rc::new(RefCell::new(TranslatedVariable {
        old_name,
        new_name: new_name.clone(),
        type_name: variable_type_name.clone(),
        abi_type_name,
        is_storage,
        is_configurable,
        is_constant,
        ..Default::default()
    })));

    // Generate a getter function if the storage field is public
    if !is_public {
        return Ok(());
    }

    // Generate parameters and return type for the public getter function
    let mut parameters = vec![];
    let mut return_type = match variable_type_name {
        sway::TypeName::StringSlice => {
            // Ensure `std::string::*` is imported
            translated_definition.ensure_use_declared("std::string::*");
    
            sway::TypeName::Identifier {
                name: "String".into(),
                generic_parameters: None,
            }
        },

        _ => variable_type_name.clone(),
    };

    if let Some((inner_parameters, inner_return_type)) = variable_type_name.getter_function_parameters_and_return_type() {
        parameters = inner_parameters;
        return_type = inner_return_type;
    }

    // Create the function declaration for the abi
    let mut sway_function = sway::Function {
        attributes: if is_storage {
            Some(sway::AttributeList {
                attributes: vec![
                    sway::Attribute {
                        name: "storage".into(),
                        parameters: Some(vec![
                            "read".into(),
                        ]),
                    },
                ],
            })
        } else {
            None
        },
        is_public: false,
        name: new_name.clone(),
        generic_parameters: None,
        parameters: sway::ParameterList {
            entries: parameters.iter().map(|(p, _)| p.clone()).collect(),
        },
        return_type: Some(return_type),
        body: None,
    };

    if let Some(abi) = translated_definition.abi.as_mut() {
        // Only add the function to the abi if it doesn't already exist
        if !abi.functions.contains(&sway_function) {
            abi.functions.push(sway_function.clone());
        }
    } else {
        // Add the function to the abi
        translated_definition.get_abi().functions.push(sway_function.clone());
    }

    // Create the body for the toplevel function
    sway_function.body = Some(sway::Block {
        statements: vec![],
        final_expr: Some(if is_storage {
            let mut expression = sway::Expression::from(sway::MemberAccess {
                expression: sway::Expression::Identifier("storage".into()),
                member: new_name.clone(),
            });

            for (parameter, needs_unwrap) in parameters.iter() {
                expression = sway::Expression::from(sway::FunctionCall {
                    function: sway::Expression::from(sway::MemberAccess {
                        expression,
                        member: "get".into(),
                    }),
                    generic_parameters: None,
                    parameters: vec![
                        sway::Expression::Identifier(parameter.name.clone()),
                    ],
                });

                if *needs_unwrap {
                    expression = sway::Expression::from(sway::FunctionCall {
                        function: sway::Expression::from(sway::MemberAccess {
                            expression,
                            member: "unwrap".into(),
                        }),
                        generic_parameters: None,
                        parameters: vec![],
                    });
                }
            }
            
            sway::Expression::from(sway::FunctionCall {
                function: sway::Expression::from(sway::MemberAccess {
                    expression,
                    member: "read".into(),
                }),
                generic_parameters: None,
                parameters: vec![],
            })
        } else if is_constant {
            sway::Expression::Identifier(new_name.clone())
        } else {
            todo!("Handle getter function for non-storage variables: {} - {variable_definition:#?}", variable_definition.to_string())
        }),
    });

    // Add the toplevel function
    translated_definition.functions.push(sway_function.clone());

    // Create the body for the contract impl's function wrapper
    sway_function.body = Some(sway::Block {
        statements: vec![],
        final_expr: Some(sway::Expression::from(sway::FunctionCall {
            function: sway::Expression::Identifier(format!("::{}", sway_function.name)),
            generic_parameters: None,
            parameters: vec![],
        })),
    });

    // Create the function wrapper item for the contract impl block
    let impl_item = sway::ImplItem::Function(sway_function);

    if let Some(contract_impl) = translated_definition.find_contract_impl_mut() {
        // Only add the function wrapper to the contract impl if it doesn't already exist
        if !contract_impl.items.contains(&impl_item) {
            contract_impl.items.push(impl_item);
        }
    } else {
        // Add the function wrapper to the contract impl
        translated_definition.get_contract_impl().items.push(impl_item);
    }

    Ok(())
}
