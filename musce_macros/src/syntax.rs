use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, Ident, LitStr, Path, Result, Token, Type, Visibility, braced, parenthesized};

mod kw {
    syn::custom_keyword!(requires);
    syn::custom_keyword!(effects);
    syn::custom_keyword!(gate);
    syn::custom_keyword!(resolution);
    syn::custom_keyword!(observe);
    syn::custom_keyword!(via);
    syn::custom_keyword!(execute);
    syn::custom_keyword!(narrate);
    syn::custom_keyword!(exists);
    syn::custom_keyword!(all);
}

pub(crate) struct Definition {
    pub(crate) visibility: Visibility,
    pub(crate) name: Ident,
    pub(crate) inputs: Vec<ParameterDecl>,
    pub(crate) results: Vec<ParameterDecl>,
    pub(crate) guards: Vec<GuardDecl>,
    pub(crate) effects: Vec<Expr>,
    pub(crate) gate: GateDecl,
    pub(crate) resolution: Ident,
    pub(crate) observation: Option<ObservationDecl>,
    pub(crate) execute: Path,
    pub(crate) narrate: Option<Path>,
}

impl Parse for Definition {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let visibility = input.parse()?;
        let name: Ident = input.parse()?;

        let parameters;
        parenthesized!(parameters in input);
        let inputs = parameters
            .parse_terminated(ParameterDecl::parse, Token![,])?
            .into_iter()
            .collect();

        let results = if input.peek(Token![->]) {
            input.parse::<Token![->]>()?;
            let parameters;
            parenthesized!(parameters in input);
            parameters
                .parse_terminated(ParameterDecl::parse, Token![,])?
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };

        let body;
        braced!(body in input);

        body.parse::<kw::requires>()?;
        let guard_body;
        braced!(guard_body in body);
        let mut guards = Vec::new();
        while !guard_body.is_empty() {
            guards.push(guard_body.parse()?);
        }

        body.parse::<kw::effects>()?;
        let effect_body;
        braced!(effect_body in body);
        let effects = effect_body
            .parse_terminated(Expr::parse, Token![;])?
            .into_iter()
            .collect();

        body.parse::<kw::gate>()?;
        let gate = body.parse()?;
        body.parse::<Token![;]>()?;

        body.parse::<kw::resolution>()?;
        let resolution = body.parse()?;
        body.parse::<Token![;]>()?;

        let observation = if body.peek(kw::observe) {
            Some(body.parse()?)
        } else {
            None
        };

        body.parse::<kw::execute>()?;
        let execute = body.parse()?;
        body.parse::<Token![;]>()?;

        let narrate = if body.peek(kw::narrate) {
            body.parse::<kw::narrate>()?;
            let path = body.parse()?;
            body.parse::<Token![;]>()?;
            Some(path)
        } else {
            None
        };

        if !body.is_empty() {
            return Err(body.error("unexpected affordance declaration item"));
        }
        if !input.is_empty() {
            return Err(input.error("expected exactly one affordance declaration"));
        }

        Ok(Self {
            visibility,
            name,
            inputs,
            results,
            guards,
            effects,
            gate,
            resolution,
            observation,
            execute,
            narrate,
        })
    }
}

pub(crate) struct ParameterDecl {
    pub(crate) name: Ident,
    pub(crate) sort: Sort,
}

impl Parse for ParameterDecl {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        let sort = input.parse()?;
        Ok(Self { name, sort })
    }
}

#[derive(Clone)]
pub(crate) enum Sort {
    Entity,
    Text,
    Symbol(LitStr),
}

impl Sort {
    fn parse_named(input: ParseStream<'_>, name: Ident) -> Result<Self> {
        match name.to_string().as_str() {
            "Entity" => Ok(Self::Entity),
            "Text" => Ok(Self::Text),
            "Symbol" => {
                let domain;
                parenthesized!(domain in input);
                let value = domain.parse()?;
                if !domain.is_empty() {
                    return Err(domain.error("Symbol expects one string-literal domain id"));
                }
                Ok(Self::Symbol(value))
            }
            _ => Err(syn::Error::new(
                name.span(),
                "expected Entity, Text, or Symbol(\"domain\")",
            )),
        }
    }

    pub(crate) fn rust_type(&self) -> TokenStream {
        match self {
            Self::Entity => quote!(::musce::world::EntityId),
            Self::Text => quote!(::std::string::String),
            Self::Symbol(_) => quote!(::musce::action::schema::SymbolValue),
        }
    }

    pub(crate) fn schema_sort(&self) -> TokenStream {
        match self {
            Self::Entity => quote!(::musce::action::schema::ValueSort::Entity),
            Self::Text => quote!(::musce::action::schema::ValueSort::Text),
            Self::Symbol(domain) => quote!(::musce::action::schema::ValueSort::Symbol(
                ::musce::action::schema::SymbolDomainId::new(#domain)
                    .expect("macro-validated symbol domain id")
            )),
        }
    }
}

impl Parse for Sort {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let name = input.parse()?;
        Self::parse_named(input, name)
    }
}

pub(crate) enum FormulaDecl {
    One(Expr),
    Block {
        local: Option<ParameterDecl>,
        conditions: Vec<Expr>,
    },
}

pub(crate) struct GuardDecl {
    pub(crate) formula: FormulaDecl,
    pub(crate) reason: LitStr,
}

impl Parse for GuardDecl {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let formula = if input.peek(kw::exists) {
            input.parse::<kw::exists>()?;
            let binder;
            parenthesized!(binder in input);
            let local: ParameterDecl = binder.parse()?;
            if !binder.is_empty() {
                return Err(binder.error("exists expects one local declaration"));
            }
            let conditions;
            braced!(conditions in input);
            FormulaDecl::Block {
                local: Some(local),
                conditions: parse_expressions(&conditions)?,
            }
        } else if input.peek(kw::all) {
            input.parse::<kw::all>()?;
            let conditions;
            braced!(conditions in input);
            FormulaDecl::Block {
                local: None,
                conditions: parse_expressions(&conditions)?,
            }
        } else {
            FormulaDecl::One(input.parse()?)
        };
        input.parse::<Token![=>]>()?;
        let reason = input.parse()?;
        input.parse::<Token![;]>()?;
        Ok(Self { formula, reason })
    }
}

fn parse_expressions(input: ParseStream<'_>) -> Result<Vec<Expr>> {
    Ok(input
        .parse_terminated(Expr::parse, Token![;])?
        .into_iter()
        .collect())
}

pub(crate) enum GateDecl {
    Open,
    Cap(Ident),
}

impl Parse for GateDecl {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let kind: Ident = input.parse()?;
        match kind.to_string().as_str() {
            "Open" => Ok(Self::Open),
            "Cap" => {
                let cap;
                parenthesized!(cap in input);
                let name = cap.parse()?;
                if !cap.is_empty() {
                    return Err(cap.error("Cap expects one constructor parameter name"));
                }
                Ok(Self::Cap(name))
            }
            _ => Err(syn::Error::new(
                kind.span(),
                "gate must be Open or Cap(parameter_name)",
            )),
        }
    }
}

pub(crate) struct ObservationDecl {
    pub(crate) ty: Type,
    pub(crate) function: Path,
}

impl Parse for ObservationDecl {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        input.parse::<kw::observe>()?;
        let ty = input.parse()?;
        input.parse::<kw::via>()?;
        let function = input.parse()?;
        input.parse::<Token![;]>()?;
        Ok(Self { ty, function })
    }
}
