//! Procedural authoring front ends for MUSCE's canonical runtime types.

use std::collections::HashMap;

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{Expr, ExprCall, ExprMethodCall, ExprPath, Ident, LitStr, Path, Result};

mod syntax;
use syntax::{Definition, FormulaDecl, GateDecl, ParameterDecl, Sort};

/// Define one app-owned affordance and lower it into MUSCE's canonical schema.
#[proc_macro]
pub fn affordance(input: TokenStream) -> TokenStream {
    match syn::parse::<Definition>(input).and_then(expand) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

struct Lowering<'a> {
    parameters: HashMap<String, (&'a ParameterDecl, ParameterMode)>,
    relations: Vec<Path>,
    relation_fields: HashMap<String, Ident>,
    components: Vec<Path>,
    component_fields: HashMap<String, Ident>,
    synthetic_local: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParameterMode {
    Input,
    Result,
}

impl<'a> Lowering<'a> {
    fn new(inputs: &'a [ParameterDecl], results: &'a [ParameterDecl]) -> Result<Self> {
        if inputs.len() > usize::from(u16::MAX) + 1 {
            return Err(syn::Error::new(
                Span::call_site(),
                "an affordance cannot declare more than 65536 inputs",
            ));
        }
        if results.len() > usize::from(u16::MAX) + 1 {
            return Err(syn::Error::new(
                Span::call_site(),
                "an affordance cannot declare more than 65536 results",
            ));
        }
        let mut parameters = HashMap::new();
        for (parameter, mode) in inputs
            .iter()
            .map(|parameter| (parameter, ParameterMode::Input))
            .chain(
                results
                    .iter()
                    .map(|parameter| (parameter, ParameterMode::Result)),
            )
        {
            let name = parameter.name.to_string();
            if name == "Actor" {
                return Err(syn::Error::new(
                    parameter.name.span(),
                    "Actor is supplied by the execution context and cannot be a parameter",
                ));
            }
            if let Sort::Symbol(domain) = &parameter.sort {
                validate_stable_name(domain, "symbol domain id")?;
            }
            if parameters.insert(name.clone(), (parameter, mode)).is_some() {
                return Err(syn::Error::new(
                    parameter.name.span(),
                    format!("duplicate parameter {name:?}"),
                ));
            }
        }
        Ok(Self {
            parameters,
            relations: Vec::new(),
            relation_fields: HashMap::new(),
            components: Vec::new(),
            component_fields: HashMap::new(),
            synthetic_local: 0,
        })
    }

    fn relation(&mut self, path: Path) -> Ident {
        let key = quote!(#path).to_string();
        if let Some(field) = self.relation_fields.get(&key) {
            return field.clone();
        }
        let field = format_ident!("__relation_{}", self.relations.len());
        self.relations.push(path);
        self.relation_fields.insert(key, field.clone());
        field
    }

    fn component(&mut self, path: Path) -> Ident {
        let key = quote!(#path).to_string();
        if let Some(field) = self.component_fields.get(&key) {
            return field.clone();
        }
        let field = format_ident!("__component_{}", self.components.len());
        self.components.push(path);
        self.component_fields.insert(key, field.clone());
        field
    }

    fn term(
        &self,
        expression: &Expr,
        locals: &HashMap<String, Sort>,
        allow_results: bool,
    ) -> Result<TokenStream2> {
        let path = expression_path(expression, "expected Actor or a declared value name")?;
        let Some(ident) = path.get_ident() else {
            return Err(syn::Error::new_spanned(
                expression,
                "terms must be Actor or an unqualified declared value name",
            ));
        };
        let name = ident.to_string();
        if name == "Actor" {
            return Ok(quote!(::musce::action::schema::Term::Actor));
        }
        if let Some((parameter, mode)) = self.parameters.get(&name) {
            if !matches!(parameter.sort, Sort::Entity) {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("{name:?} is not entity-valued"),
                ));
            }
            return match mode {
                ParameterMode::Input => Ok(quote!(::musce::action::schema::Term::Input(
                    ::musce::action::schema::ParameterId::new(#name)
                        .expect("macro-validated input id")
                ))),
                ParameterMode::Result if allow_results => {
                    Ok(quote!(::musce::action::schema::Term::Result(
                        ::musce::action::schema::ParameterId::new(#name)
                            .expect("macro-validated result id")
                    )))
                }
                ParameterMode::Result => Err(syn::Error::new(
                    ident.span(),
                    "result parameters cannot appear in requirements",
                )),
            };
        }
        if let Some(sort) = locals.get(&name) {
            if allow_results {
                return Err(syn::Error::new(
                    ident.span(),
                    "formula locals cannot appear in effects",
                ));
            }
            if !matches!(sort, Sort::Entity) {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("local {name:?} is not entity-valued"),
                ));
            }
            return Ok(quote!(::musce::action::schema::Term::Local(
                ::musce::action::schema::LocalId::new(#name)
                    .expect("macro-validated local id")
            )));
        }
        Err(syn::Error::new(
            ident.span(),
            format!("undeclared value {name:?}"),
        ))
    }

    fn lower_condition(
        &mut self,
        expression: &Expr,
        locals: &HashMap<String, Sort>,
        negated: bool,
    ) -> Result<LoweredConditions> {
        if let Expr::Call(call) = unparen(expression) {
            let function = expression_path(&call.func, "expected a condition function")?;
            let Some(name) = function.get_ident() else {
                return Err(syn::Error::new_spanned(
                    function,
                    "condition functions must be unqualified",
                ));
            };
            match name.to_string().as_str() {
                "not" => {
                    let argument = one_argument(call, "not")?;
                    return self.lower_condition(argument, locals, !negated);
                }
                "distinct" => {
                    if negated {
                        return Err(syn::Error::new_spanned(
                            expression,
                            "the canonical algebra has no equality condition",
                        ));
                    }
                    let [left, right] = two_arguments(call, "distinct")?;
                    let left = self.term(left, locals, false)?;
                    let right = self.term(right, locals, false)?;
                    return Ok(LoweredConditions::one(quote!(
                        ::musce::action::schema::Condition::Distinct {
                            left: #left,
                            right: #right,
                        }
                    )));
                }
                "same_locus" => {
                    if negated {
                        return Err(syn::Error::new_spanned(
                            expression,
                            "negated same_locus has no canonical expansion",
                        ));
                    }
                    let [left, right] = two_arguments(call, "same_locus")?;
                    let left = self.term(left, locals, false)?;
                    let right = self.term(right, locals, false)?;
                    let local_name = format!("__musce_same_locus_{}", self.synthetic_local);
                    self.synthetic_local += 1;
                    let local_id = quote!(::musce::action::schema::LocalId::new(#local_name)
                        .expect("generated local id"));
                    return Ok(LoweredConditions {
                        locals: vec![quote!(::musce::action::schema::Local::new(
                            #local_id,
                            ::musce::action::schema::ValueSort::Entity,
                        ))],
                        conditions: vec![
                            quote!(::musce::action::schema::Condition::LocusOf {
                                entity: #left,
                                locus: ::musce::action::schema::OptionalEntity::Is(
                                    ::musce::action::schema::Term::Local(
                                        ::musce::action::schema::LocalId::new(#local_name)
                                            .expect("generated local id")
                                    )
                                ),
                            }),
                            quote!(::musce::action::schema::Condition::LocusOf {
                                entity: #right,
                                locus: ::musce::action::schema::OptionalEntity::Is(
                                    ::musce::action::schema::Term::Local(
                                        ::musce::action::schema::LocalId::new(#local_name)
                                            .expect("generated local id")
                                    )
                                ),
                            }),
                        ],
                    });
                }
                _ => {}
            }
        }

        let Expr::MethodCall(call) = unparen(expression) else {
            return Err(syn::Error::new_spanned(
                expression,
                "unsupported condition; use the closed affordance vocabulary",
            ));
        };
        self.lower_method_condition(call, locals, negated)
    }

    fn lower_method_condition(
        &mut self,
        call: &ExprMethodCall,
        locals: &HashMap<String, Sort>,
        negated: bool,
    ) -> Result<LoweredConditions> {
        let method = call.method.to_string();
        let entity = self.term(&call.receiver, locals, false)?;
        let args: Vec<_> = call.args.iter().collect();
        let condition = match method.as_str() {
            "relation_is" | "relation_is_not" => {
                require_argument_count(call, 2)?;
                let relation = expression_path(args[0], "expected a relation type path")?.clone();
                let field = self.relation(relation);
                let target = self.term(args[1], locals, false)?;
                let is_not = (method == "relation_is_not") ^ negated;
                let target = if is_not {
                    quote!(::musce::action::schema::OptionalEntity::IsNot(#target))
                } else {
                    quote!(::musce::action::schema::OptionalEntity::Is(#target))
                };
                quote!(::musce::action::schema::Condition::RelationTarget {
                    source: #entity,
                    relation: self.#field.clone(),
                    target: #target,
                })
            }
            "has_no_relation" => {
                require_argument_count(call, 1)?;
                if negated {
                    return Err(syn::Error::new_spanned(
                        call,
                        "negating has_no_relation cannot name the required target",
                    ));
                }
                let relation = expression_path(args[0], "expected a relation type path")?.clone();
                let field = self.relation(relation);
                quote!(::musce::action::schema::Condition::RelationTarget {
                    source: #entity,
                    relation: self.#field.clone(),
                    target: ::musce::action::schema::OptionalEntity::IsUnset,
                })
            }
            "has_component" | "has_no_component" => {
                require_argument_count(call, 1)?;
                let component = expression_path(args[0], "expected a component type path")?.clone();
                let field = self.component(component);
                let mut present = method == "has_component";
                if negated {
                    present = !present;
                }
                quote!(::musce::action::schema::Condition::ComponentPresent {
                    entity: #entity,
                    component: self.#field.clone(),
                    present: #present,
                })
            }
            "at_locus" | "not_at_locus" => {
                require_argument_count(call, 1)?;
                let locus = self.term(args[0], locals, false)?;
                let is_not = (method == "not_at_locus") ^ negated;
                let locus = if is_not {
                    quote!(::musce::action::schema::OptionalEntity::IsNot(#locus))
                } else {
                    quote!(::musce::action::schema::OptionalEntity::Is(#locus))
                };
                quote!(::musce::action::schema::Condition::LocusOf {
                    entity: #entity,
                    locus: #locus,
                })
            }
            "has_no_locus" => {
                require_argument_count(call, 0)?;
                if negated {
                    return Err(syn::Error::new_spanned(
                        call,
                        "negating has_no_locus cannot bind the required locus",
                    ));
                }
                quote!(::musce::action::schema::Condition::LocusOf {
                    entity: #entity,
                    locus: ::musce::action::schema::OptionalEntity::IsUnset,
                })
            }
            "gauge_at_least" | "gauge_at_most" => {
                require_argument_count(call, 2)?;
                if negated {
                    return Err(syn::Error::new_spanned(
                        call,
                        "negated gauge bounds are not canonical; name the opposite registered bound",
                    ));
                }
                let gauge = string_literal(args[0], "gauge id")?;
                let region = string_literal(args[1], "gauge region id")?;
                let variant = if method == "gauge_at_least" {
                    quote!(GaugeAtLeast)
                } else {
                    quote!(GaugeAtMost)
                };
                quote!(::musce::action::schema::Condition::#variant {
                    entity: #entity,
                    gauge: ::musce::action::GaugeId::new(#gauge),
                    region: ::musce::action::schema::GaugeRegionId::new(#region)
                        .expect("macro-validated gauge region id"),
                })
            }
            "exists" | "does_not_exist" => {
                require_argument_count(call, 0)?;
                let mut exists = method == "exists";
                if negated {
                    exists = !exists;
                }
                if exists && self.is_input_term(&call.receiver) {
                    return Err(syn::Error::new_spanned(
                        call,
                        "positive existence tests on entity inputs are implicit and not authorable",
                    ));
                }
                quote!(::musce::action::schema::Condition::Exists {
                    entity: #entity,
                    exists: #exists,
                })
            }
            _ => {
                return Err(syn::Error::new(
                    call.method.span(),
                    format!("unsupported affordance condition method {method:?}"),
                ));
            }
        };
        Ok(LoweredConditions::one(condition))
    }

    fn is_input_term(&self, expression: &Expr) -> bool {
        expression_path(expression, "")
            .ok()
            .and_then(Path::get_ident)
            .and_then(|ident| self.parameters.get(&ident.to_string()))
            .is_some_and(|(_, mode)| *mode == ParameterMode::Input)
    }

    fn lower_effect(&mut self, expression: &Expr) -> Result<TokenStream2> {
        let Expr::MethodCall(call) = unparen(expression) else {
            return Err(syn::Error::new_spanned(
                expression,
                "effects must use the closed affordance effect vocabulary",
            ));
        };
        let locals = HashMap::new();
        let entity = self.term(&call.receiver, &locals, true)?;
        let args: Vec<_> = call.args.iter().collect();
        let method = call.method.to_string();
        match method.as_str() {
            "set_relation" => {
                require_argument_count(call, 2)?;
                let relation = expression_path(args[0], "expected a relation type path")?.clone();
                let field = self.relation(relation);
                let target = self.term(args[1], &locals, true)?;
                Ok(quote!(::musce::action::schema::Effect::SetRelation {
                    source: #entity,
                    relation: self.#field.clone(),
                    target: #target,
                }))
            }
            "clear_relation" => {
                require_argument_count(call, 1)?;
                let relation = expression_path(args[0], "expected a relation type path")?.clone();
                let field = self.relation(relation);
                Ok(quote!(::musce::action::schema::Effect::ClearRelation {
                    source: #entity,
                    relation: self.#field.clone(),
                }))
            }
            "set_component" | "remove_component" => {
                require_argument_count(call, 1)?;
                let component = expression_path(args[0], "expected a component type path")?.clone();
                let field = self.component(component);
                let variant = if method == "set_component" {
                    quote!(SetComponent)
                } else {
                    quote!(RemoveComponent)
                };
                Ok(quote!(::musce::action::schema::Effect::#variant {
                    entity: #entity,
                    component: self.#field.clone(),
                }))
            }
            "set_locus" => {
                require_argument_count(call, 1)?;
                let locus = self.term(args[0], &locals, true)?;
                Ok(quote!(::musce::action::schema::Effect::SetLocus {
                    entity: #entity,
                    locus: #locus,
                }))
            }
            "clear_locus" => {
                require_argument_count(call, 0)?;
                Ok(quote!(::musce::action::schema::Effect::ClearLocus {
                    entity: #entity,
                }))
            }
            "shift_gauge" => {
                require_argument_count(call, 2)?;
                let gauge = string_literal(args[0], "gauge id")?;
                let direction = expression_path(args[1], "expected Up or Down")?;
                let Some(direction) = direction.get_ident() else {
                    return Err(syn::Error::new_spanned(direction, "expected Up or Down"));
                };
                let direction = match direction.to_string().as_str() {
                    "Up" => quote!(::musce::action::GaugeDirection::Up),
                    "Down" => quote!(::musce::action::GaugeDirection::Down),
                    _ => return Err(syn::Error::new(direction.span(), "expected Up or Down")),
                };
                Ok(quote!(::musce::action::schema::Effect::ShiftGauge {
                    entity: #entity,
                    gauge: ::musce::action::GaugeId::new(#gauge),
                    direction: #direction,
                }))
            }
            "create" => {
                require_argument_count(call, 0)?;
                let path = expression_path(&call.receiver, "create must name a result parameter")?;
                let Some(result) = path.get_ident() else {
                    return Err(syn::Error::new_spanned(
                        path,
                        "create must name a result parameter",
                    ));
                };
                let name = result.to_string();
                match self.parameters.get(&name) {
                    Some((parameter, ParameterMode::Result))
                        if matches!(parameter.sort, Sort::Entity) => {}
                    _ => {
                        return Err(syn::Error::new(
                            result.span(),
                            "create must name an entity result parameter",
                        ));
                    }
                }
                Ok(quote!(::musce::action::schema::Effect::Create {
                    result: ::musce::action::schema::ParameterId::new(#name)
                        .expect("macro-validated result id"),
                }))
            }
            "destroy" => {
                require_argument_count(call, 0)?;
                Ok(quote!(::musce::action::schema::Effect::Destroy {
                    entity: #entity,
                }))
            }
            _ => Err(syn::Error::new(
                call.method.span(),
                format!("unsupported affordance effect method {method:?}"),
            )),
        }
    }
}

struct LoweredConditions {
    locals: Vec<TokenStream2>,
    conditions: Vec<TokenStream2>,
}

impl LoweredConditions {
    fn one(condition: TokenStream2) -> Self {
        Self {
            locals: Vec::new(),
            conditions: vec![condition],
        }
    }
}

fn expand(definition: Definition) -> Result<TokenStream2> {
    validate_identifier(&definition.name, "affordance")?;
    let mut lowering = Lowering::new(&definition.inputs, &definition.results)?;

    let mut guards = Vec::new();
    for guard in &definition.guards {
        let mut locals = HashMap::new();
        let expressions = match &guard.formula {
            FormulaDecl::One(expression) => vec![expression],
            FormulaDecl::Block { local, conditions } => {
                if let Some(local) = local {
                    validate_identifier(&local.name, "local")?;
                    if local.name.to_string().starts_with("__musce_") {
                        return Err(syn::Error::new(
                            local.name.span(),
                            "local names beginning with __musce_ are reserved",
                        ));
                    }
                    if matches!(local.sort, Sort::Text) {
                        return Err(syn::Error::new(
                            local.name.span(),
                            "Text locals are not enumerable",
                        ));
                    }
                    let name = local.name.to_string();
                    if lowering.parameters.contains_key(&name) {
                        return Err(syn::Error::new(
                            local.name.span(),
                            "a formula local cannot shadow a parameter",
                        ));
                    }
                    locals.insert(name, local.sort.clone());
                }
                conditions.iter().collect()
            }
        };

        let mut formula_locals = Vec::new();
        if let FormulaDecl::Block {
            local: Some(local), ..
        } = &guard.formula
        {
            let name = local.name.to_string();
            let sort = local.sort.schema_sort();
            formula_locals.push(quote!(::musce::action::schema::Local::new(
                ::musce::action::schema::LocalId::new(#name)
                    .expect("macro-validated local id"),
                #sort,
            )));
        }
        let mut conditions = Vec::new();
        for expression in expressions {
            let lowered = lowering.lower_condition(expression, &locals, false)?;
            formula_locals.extend(lowered.locals);
            conditions.extend(lowered.conditions);
        }
        let reason = &guard.reason;
        guards.push(quote!(::musce::action::schema::Guard::new(
            ::musce::action::schema::Formula::new(
                vec![#(#formula_locals),*],
                vec![#(#conditions),*],
            ),
            #reason,
        )));
    }

    let mut effects = Vec::new();
    for effect in &definition.effects {
        effects.push(lowering.lower_effect(effect)?);
    }

    let resolution = match definition.resolution.to_string().as_str() {
        "Deterministic" => quote!(::musce::action::schema::Resolution::Deterministic),
        "Contested" => quote!(::musce::action::schema::Resolution::Contested),
        "Opaque" => quote!(::musce::action::schema::Resolution::Opaque),
        _ => {
            return Err(syn::Error::new(
                definition.resolution.span(),
                "resolution must be Deterministic, Contested, or Opaque",
            ));
        }
    };

    let visibility = &definition.visibility;
    let name = &definition.name;
    let affordance_name = name.to_string();
    let type_name = format_ident!("{}", pascal_case(&affordance_name));
    let inputs_name = format_ident!("{}Inputs", type_name);
    let results_name = format_ident!("{}Results", type_name);
    let const_name = format_ident!("{}", affordance_name.to_uppercase());
    let action_name = format_ident!("{}_action", affordance_name);
    let display_name = pascal_case(&affordance_name);

    let input_fields = definition.inputs.iter().map(|parameter| {
        let name = &parameter.name;
        let ty = parameter.sort.rust_type();
        quote!(#visibility #name: #ty)
    });
    let result_fields = definition.results.iter().map(|parameter| {
        let name = &parameter.name;
        let ty = parameter.sort.rust_type();
        quote!(#visibility #name: #ty)
    });

    let schema_parameters = definition
        .inputs
        .iter()
        .enumerate()
        .map(|(slot, parameter)| schema_parameter(parameter, slot, ParameterMode::Input))
        .chain(
            definition
                .results
                .iter()
                .enumerate()
                .map(|(slot, parameter)| schema_parameter(parameter, slot, ParameterMode::Result)),
        );

    let input_names: Vec<_> = definition.inputs.iter().map(|p| &p.name).collect();
    let action_arguments = definition.inputs.iter().map(|parameter| {
        let name = &parameter.name;
        let ty = parameter.sort.rust_type();
        quote!(#name: #ty)
    });
    let decoded_inputs = definition.inputs.iter().map(decode_input);
    let input_values = definition.inputs.iter().map(|parameter| {
        let field = &parameter.name;
        encode_value(&parameter.sort, quote!(#field))
    });
    let result_values = definition.results.iter().map(|parameter| {
        let field = &parameter.name;
        encode_value(&parameter.sort, quote!(results.#field))
    });

    // HashMap iteration would make expansion nondeterministic; field order follows dependency order.
    let relation_fields: Vec<_> = (0..lowering.relations.len())
        .map(|index| format_ident!("__relation_{index}"))
        .collect();
    let component_fields: Vec<_> = (0..lowering.components.len())
        .map(|index| format_ident!("__component_{index}"))
        .collect();
    let relation_types = &lowering.relations;
    let component_types = &lowering.components;

    let (cap_field, cap_argument, gate) = match &definition.gate {
        GateDecl::Open => (quote!(), quote!(), quote!(::musce::action::Gate::Open)),
        GateDecl::Cap(cap) => (
            quote!(__gate_cap: ::musce::action::CapId,),
            quote!(, #cap: ::musce::action::CapId),
            quote!(::musce::action::Gate::Cap(self.__gate_cap)),
        ),
    };
    let cap_init = match &definition.gate {
        GateDecl::Open => quote!(),
        GateDecl::Cap(cap) => quote!(__gate_cap: #cap,),
    };

    let (observations_type, observe) = match &definition.observation {
        Some(observation) => {
            let ty = &observation.ty;
            let function = &observation.function;
            (quote!(#ty), quote!(#function(world, actor, inputs)))
        }
        None => (quote!(()), quote!({})),
    };
    let execute = &definition.execute;
    let narrate = match &definition.narrate {
        Some(function) => quote!(#function(ctx, inputs, results, observations);),
        None => quote! {
            let _ = (ctx, inputs, results, observations);
        },
    };

    Ok(quote! {
        #visibility const #const_name: &str = #affordance_name;

        #[derive(Clone, Debug, PartialEq, Eq)]
        #visibility struct #inputs_name {
            #(#input_fields,)*
        }

        #[derive(Clone, Debug, PartialEq, Eq)]
        #visibility struct #results_name {
            #(#result_fields,)*
        }

        #visibility struct #type_name {
            #(#relation_fields: ::musce::action::schema::RelationId,)*
            #(#component_fields: ::musce::action::schema::ComponentId,)*
            #cap_field
        }

        impl #type_name {
            #visibility fn register(
                state: &mut ::musce::action::state::StateRegistry
                #cap_argument
            ) -> ::std::result::Result<
                Self,
                ::musce::action::state::StateRegistrationError,
            > {
                Ok(Self {
                    #(#relation_fields: state.register_relation::<#relation_types>()?,)*
                    #(#component_fields: state.register_component::<#component_types>()?,)*
                    #cap_init
                })
            }
        }

        #visibility fn #action_name(
            actor: ::musce::world::EntityId,
            #(#action_arguments,)*
        ) -> ::musce::action::schema::GroundAction {
            ::musce::action::schema::GroundAction::new(
                ::musce::action::schema::AffordanceId::new(#const_name)
                    .expect("macro-validated affordance id"),
                actor,
                vec![#(#input_values),*],
            )
        }

        impl ::musce::action::AffordanceDefinition for #type_name {
            type Inputs = #inputs_name;
            type Results = #results_name;
            type Observations = #observations_type;

            fn schema(&self) -> ::musce::action::schema::AffordanceSchema {
                ::musce::action::schema::AffordanceSchema::new(
                    ::musce::action::schema::AffordanceId::new(#const_name)
                        .expect("macro-validated affordance id"),
                    #display_name,
                    vec![#(#schema_parameters),*],
                    vec![#(#guards),*],
                    vec![#(#effects),*],
                    #gate,
                    #resolution,
                )
            }

            fn decode_inputs(
                &self,
                action: &::musce::action::schema::GroundAction,
            ) -> ::std::result::Result<Self::Inputs, ::musce::action::AdapterError> {
                let [#(#input_names),*] = action.inputs() else {
                    return Err(::musce::action::AdapterError::new(concat!(
                        #affordance_name,
                        " received the wrong number of inputs",
                    )));
                };
                Ok(#inputs_name {
                    #(#decoded_inputs,)*
                })
            }

            fn observe(
                &self,
                world: &::musce::world::World,
                actor: ::musce::world::EntityId,
                inputs: &Self::Inputs,
            ) -> Self::Observations {
                #observe
            }

            fn execute(
                &self,
                ctx: &mut ::musce::action::PerformCtx<'_>,
                inputs: &Self::Inputs,
            ) -> ::musce::action::TypedHandlerOutcome<Self::Results> {
                #execute(ctx, inputs)
            }

            fn encode_results(
                &self,
                results: &Self::Results,
            ) -> ::musce::action::schema::ActionOutcome {
                ::musce::action::schema::ActionOutcome::new(vec![#(#result_values),*])
            }

            fn narrate(
                &self,
                ctx: &mut ::musce::action::NarrationCtx<'_>,
                inputs: &Self::Inputs,
                results: &Self::Results,
                observations: &Self::Observations,
            ) {
                #narrate
            }
        }
    })
}

fn schema_parameter(parameter: &ParameterDecl, slot: usize, mode: ParameterMode) -> TokenStream2 {
    let name = parameter.name.to_string();
    let sort = parameter.sort.schema_sort();
    let mode = match mode {
        ParameterMode::Input => quote!(::musce::action::schema::ParameterMode::Input),
        ParameterMode::Result => quote!(::musce::action::schema::ParameterMode::Result),
    };
    let slot = slot as u16;
    quote!(::musce::action::schema::Parameter::new(
        ::musce::action::schema::ParameterId::new(#name)
            .expect("macro-validated parameter id"),
        #name,
        #sort,
        #mode,
        #slot,
    ).expect("macro-validated parameter declaration"))
}

fn decode_input(parameter: &ParameterDecl) -> TokenStream2 {
    let name = &parameter.name;
    let label = name.to_string();
    match &parameter.sort {
        Sort::Entity => quote!(
            #name: #name.as_entity().ok_or_else(||
                ::musce::action::AdapterError::new(concat!(#label, " was not an entity"))
            )?
        ),
        Sort::Text => quote!(
            #name: match #name {
                ::musce::action::schema::Value::Text(value) => value.to_string(),
                _ => return Err(::musce::action::AdapterError::new(concat!(
                    #label,
                    " was not text",
                ))),
            }
        ),
        Sort::Symbol(domain) => quote!(
            #name: match #name {
                ::musce::action::schema::Value::Symbol(value)
                    if value.domain().as_str() == #domain => value.clone(),
                _ => return Err(::musce::action::AdapterError::new(concat!(
                    #label,
                    " was not a member of its declared symbol domain",
                ))),
            }
        ),
    }
}

fn encode_value(sort: &Sort, value: TokenStream2) -> TokenStream2 {
    match sort {
        Sort::Entity => quote!(::musce::action::schema::Value::Entity(#value)),
        Sort::Text => quote!(::musce::action::schema::Value::text(#value.clone())),
        Sort::Symbol(_) => quote!(::musce::action::schema::Value::Symbol(#value.clone())),
    }
}

fn expression_path<'a>(expression: &'a Expr, message: &str) -> Result<&'a Path> {
    match unparen(expression) {
        Expr::Path(ExprPath {
            qself: None, path, ..
        }) => Ok(path),
        _ => Err(syn::Error::new_spanned(expression, message)),
    }
}

fn unparen(expression: &Expr) -> &Expr {
    match expression {
        Expr::Paren(paren) => unparen(&paren.expr),
        _ => expression,
    }
}

fn one_argument<'a>(call: &'a ExprCall, function: &str) -> Result<&'a Expr> {
    if call.args.len() != 1 {
        return Err(syn::Error::new_spanned(
            call,
            format!("{function} expects one argument"),
        ));
    }
    Ok(&call.args[0])
}

fn two_arguments<'a>(call: &'a ExprCall, function: &str) -> Result<[&'a Expr; 2]> {
    if call.args.len() != 2 {
        return Err(syn::Error::new_spanned(
            call,
            format!("{function} expects two arguments"),
        ));
    }
    Ok([&call.args[0], &call.args[1]])
}

fn require_argument_count(call: &ExprMethodCall, expected: usize) -> Result<()> {
    if call.args.len() == expected {
        Ok(())
    } else {
        Err(syn::Error::new_spanned(
            call,
            format!("{} expects {expected} argument(s)", call.method),
        ))
    }
}

fn string_literal<'a>(expression: &'a Expr, kind: &str) -> Result<&'a LitStr> {
    match unparen(expression) {
        Expr::Lit(literal) => match &literal.lit {
            syn::Lit::Str(value) => {
                validate_stable_name(value, kind)?;
                Ok(value)
            }
            _ => Err(syn::Error::new_spanned(
                expression,
                format!("{kind} must be a string literal"),
            )),
        },
        _ => Err(syn::Error::new_spanned(
            expression,
            format!("{kind} must be a string literal"),
        )),
    }
}

fn validate_identifier(identifier: &Ident, kind: &str) -> Result<()> {
    validate_stable_text(&identifier.to_string(), identifier.span(), kind)
}

fn validate_stable_name(value: &LitStr, kind: &str) -> Result<()> {
    validate_stable_text(&value.value(), value.span(), kind)
}

fn validate_stable_text(value: &str, span: Span, kind: &str) -> Result<()> {
    if value.is_empty() || value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        Err(syn::Error::new(
            span,
            format!("{kind} must be nonempty and contain no whitespace or control characters"),
        ))
    } else {
        Ok(())
    }
}

fn pascal_case(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().chain(characters).collect(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expands(source: &str) -> Result<TokenStream2> {
        expand(syn::parse_str(source)?)
    }

    #[test]
    fn rejects_duplicate_parameter_names() {
        let error = expands(
            r#"
            sample(item: Entity) -> (item: Entity) {
                requires {}
                effects {}
                gate Open;
                resolution Deterministic;
                execute execute_sample;
            }
            "#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("duplicate parameter"));
    }

    #[test]
    fn rejects_result_use_in_requirements() {
        let error = expands(
            r#"
            sample() -> (product: Entity) {
                requires { product.has_component(Item) => "missing"; }
                effects { product.create(); }
                gate Open;
                resolution Deterministic;
                execute execute_sample;
            }
            "#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("result parameters"));
    }

    #[test]
    fn rejects_undeclared_terms() {
        let error = expands(
            r#"
            sample(item: Entity) {
                requires { missing.has_component(Item) => "missing"; }
                effects {}
                gate Open;
                resolution Deterministic;
                execute execute_sample;
            }
            "#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("undeclared value"));
    }

    #[test]
    fn lowers_the_documented_closed_vocabulary() {
        expands(
            r#"
            sample(item: Entity, support: Entity, text: Text, mode: Symbol("modes"))
                -> (product: Entity) {
                requires {
                    item.relation_is(Containment, Actor) => "held";
                    not(item.has_component(Locked)) => "locked";
                    same_locus(Actor, support) => "far";
                    exists(locus: Entity) {
                        Actor.at_locus(locus);
                        support.at_locus(locus);
                    } => "far";
                }
                effects {
                    item.set_relation(Containment, support);
                    item.set_component(Mounted);
                    item.shift_gauge("health", Down);
                    product.create();
                }
                gate Cap(build);
                resolution Contested;
                observe Observations via observe_sample;
                execute execute_sample;
                narrate narrate_sample;
            }
            "#,
        )
        .unwrap();
    }
}
