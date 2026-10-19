//! Expression types and the member interfaces established by values or native metadata.

use std::collections::BTreeMap;

use anyhow::Result;
use tola_typst::typst::foundations::{
    Array, CastInfo, Content, Dict, Element, Styles, Type, Value, fields_on,
};

/// Only `Known` carries an actual value; instance and container types never manufacture one.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::query) enum ExpressionType {
    Unknown,
    Known(Value),
    Instance(Type),
    ElementContent(Element),
    Dictionary(BTreeMap<String, ExpressionType>),
    Tuple(Vec<ExpressionType>),
}

impl ExpressionType {
    pub(in crate::query) fn value(&self) -> Option<&Value> {
        match self {
            Self::Known(value) => Some(value),
            _ => None,
        }
    }

    pub(in crate::query) fn into_value(self) -> Option<Value> {
        match self {
            Self::Known(value) => Some(value),
            _ => None,
        }
    }

    pub(in crate::query) fn ty(&self) -> Option<Type> {
        Some(match self {
            Self::Unknown => return None,
            Self::Known(value) => value.ty(),
            Self::Instance(ty) => *ty,
            Self::ElementContent(_) => Type::of::<Content>(),
            Self::Dictionary(_) => Type::of::<Dict>(),
            Self::Tuple(_) => Type::of::<Array>(),
        })
    }

    pub(in crate::query) fn type_name(&self) -> String {
        match self {
            Self::ElementContent(element) => element.name().to_owned(),
            _ => self
                .ty()
                .map(|ty| tola_packages::type_spelling(&ty))
                .unwrap_or_else(|| "any".into()),
        }
    }

    pub(in crate::query) fn tuple(
        items: Vec<Self>,
        ensure_active: impl Fn() -> Result<()>,
    ) -> Result<Self> {
        let mut known = true;
        for ty in &items {
            ensure_active()?;
            known &= matches!(ty, Self::Known(_));
        }
        if known {
            let mut array = Array::new();
            for ty in items {
                ensure_active()?;
                array.push(ty.into_value().unwrap());
            }
            Ok(Self::Known(Value::Array(array)))
        } else {
            Ok(Self::Tuple(items))
        }
    }

    pub(in crate::query) fn field(
        &self,
        name: &str,
        ensure_active: impl Fn() -> Result<()>,
    ) -> Result<Option<Self>> {
        ensure_active()?;
        match self {
            Self::Known(value) => Ok(value.field(name, ()).ok().map(Self::Known)),
            Self::Dictionary(fields) => Ok(fields.get(name).cloned()),
            _ => Ok(members(self, None, ensure_active)?
                .remove(name)
                .filter(|member| member.call.is_none())
                .map(|member| member.ty)),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::query) enum MemberCall {
    Direct,
    Receiver,
}

pub(in crate::query) struct ExpressionMember {
    pub ty: ExpressionType,
    pub call: Option<MemberCall>,
}

/// Values expose realized fields and styles; types expose only metadata established interfaces.
pub(in crate::query) fn members(
    ty: &ExpressionType,
    styles: Option<&Styles>,
    ensure_active: impl Fn() -> Result<()>,
) -> Result<BTreeMap<String, ExpressionMember>> {
    ensure_active()?;
    let mut members = BTreeMap::new();
    let Some(instance) = ty.ty() else {
        return Ok(members);
    };
    let element = match ty {
        ExpressionType::ElementContent(element) => Some(*element),
        ExpressionType::Known(Value::Content(content)) => Some(content.elem()),
        _ => None,
    };
    for scope in element
        .into_iter()
        .map(|element| element.scope())
        .chain(Some(instance.scope()))
    {
        for (name, binding) in scope.iter() {
            ensure_active()?;
            if let Value::Func(func) = binding.read()
                && func.params().next().is_some_and(|param| {
                    param.name() == Some("self")
                        || param.to_native().is_some_and(|parameter| {
                            parameter.positional && accepts_receiver(&parameter.input, instance)
                        })
                })
            {
                members.insert(
                    name.to_string(),
                    ExpressionMember {
                        ty: ExpressionType::Known(Value::Func(func.clone())),
                        call: Some(MemberCall::Receiver),
                    },
                );
            }
        }
    }
    if let ExpressionType::Known(value) = ty {
        if let Some(scope) = value.scope() {
            for (name, binding) in scope.iter() {
                ensure_active()?;
                members.insert(
                    name.to_string(),
                    ExpressionMember {
                        ty: ExpressionType::Known(binding.read().clone()),
                        call: Some(MemberCall::Direct),
                    },
                );
            }
        }
        for field in fields_on(instance) {
            ensure_active()?;
            if let Ok(value) = value.field(field, ()) {
                members.insert(
                    (*field).into(),
                    ExpressionMember {
                        ty: ExpressionType::Known(value),
                        call: None,
                    },
                );
            }
        }
        let fields = match value {
            Value::Dict(dict) => Some(dict.clone()),
            Value::Content(content) => Some(content.fields()),
            Value::Args(args) => Some(args.to_named()),
            _ => None,
        };
        if let Some(fields) = fields {
            for (name, value) in fields {
                ensure_active()?;
                members.insert(
                    name.to_string(),
                    ExpressionMember {
                        ty: ExpressionType::Known(value),
                        call: None,
                    },
                );
            }
        }
        if let Value::Func(func) = value
            && let Some((element, styles)) = func.to_element().zip(styles)
        {
            for param in element.params() {
                ensure_active()?;
                if let Some(id) = element.field_id(param.name)
                    && let Ok(value) = element.field_from_styles(
                        id,
                        tola_typst::typst::foundations::StyleChain::new(styles),
                    )
                {
                    members.insert(
                        param.name.into(),
                        ExpressionMember {
                            ty: ExpressionType::Known(value),
                            call: None,
                        },
                    );
                }
            }
        }
    } else {
        for field in fields_on(instance) {
            ensure_active()?;
            members.insert(
                (*field).into(),
                ExpressionMember {
                    ty: ExpressionType::Unknown,
                    call: None,
                },
            );
        }
    }
    match ty {
        ExpressionType::Dictionary(fields) => {
            for (name, ty) in fields {
                ensure_active()?;
                members.insert(
                    name.clone(),
                    ExpressionMember {
                        ty: ty.clone(),
                        call: None,
                    },
                );
            }
        }
        ExpressionType::ElementContent(element) => {
            for param in element.params().iter().filter(|param| param.required) {
                ensure_active()?;
                if element.field_id(param.name).is_some() {
                    members.insert(
                        param.name.into(),
                        ExpressionMember {
                            ty: single_value(return_types(&param.input))
                                .unwrap_or(ExpressionType::Unknown),
                            call: None,
                        },
                    );
                }
            }
        }
        _ => {}
    }
    ensure_active()?;
    Ok(members)
}

fn accepts_receiver(parameter: &CastInfo, receiver: Type) -> bool {
    match parameter {
        CastInfo::Any => true,
        CastInfo::Value(value, _) => value.ty() == receiver,
        CastInfo::Type(ty) => *ty == receiver,
        CastInfo::Union(parameters) => parameters
            .iter()
            .any(|parameter| accepts_receiver(parameter, receiver)),
    }
}

pub(in crate::query) fn single_value<T: PartialEq>(mut values: Vec<T>) -> Option<T> {
    let value = values.pop()?;
    values
        .iter()
        .all(|alternative| alternative == &value)
        .then_some(value)
}

pub(in crate::query) fn native_returns(value: &Value) -> Vec<ExpressionType> {
    let function = match value {
        Value::Func(function) => function.clone(),
        Value::Type(ty) => match ty.constructor() {
            Ok(function) => function,
            Err(_) => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    if let Some(element) = function.to_element() {
        return vec![ExpressionType::ElementContent(element)];
    }
    function.returns().map(return_types).unwrap_or_default()
}

fn return_types(returns: &CastInfo) -> Vec<ExpressionType> {
    match returns {
        CastInfo::Any => Vec::new(),
        CastInfo::Value(value, _) => vec![ExpressionType::Known(value.clone())],
        CastInfo::Type(ty) => vec![ExpressionType::Instance(*ty)],
        CastInfo::Union(returns) => {
            let mut types = Vec::new();
            for returned in returns {
                let alternatives = return_types(returned);
                if alternatives.is_empty() {
                    return Vec::new();
                }
                types.extend(alternatives);
            }
            types
        }
    }
}
