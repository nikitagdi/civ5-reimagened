//! Typing of one element's bindings and actions against the model schema.

use super::{ProfileError, codes};
use crate::profile::action::{Action, AnchorKind, Arg, ControlKind};
use crate::profile::attrs::{AttrCtx, NodeAttrs};
use crate::profile::expr::{
    self, Analysis, Expr, ExprError, Scope, TplSeg, Ty, analyze, check_template,
};
use crate::profile::{Profile, SchemaView, TypeSig, ValueKind, resolve_path};

/// What checking one element found out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Checked {
    /// The scope of the element's children: the loop's item and index added.
    pub scope: Scope,
    /// The model paths the element reads, ascending, with the schema's types.
    pub paths: Vec<(String, TypeSig)>,
    /// The catalogue intents the element sends, with their argument types in declaration order.
    pub intents: Vec<(String, Vec<TypeSig>)>,
    /// The script handlers the element calls.
    pub handlers: Vec<String>,
}

struct C<'a> {
    scope: Scope,
    view: &'a dyn SchemaView,
    ctx: &'a AttrCtx,
    out: Checked,
    errs: Vec<ProfileError>,
}

/// Types the expressions, actions and paths of `attrs` in `scope` and rewrites what the schema
/// determines: positional intent arguments become named ones (in declaration order) and a loop
/// without `data-bind-key` takes the schema's list key. The returned scope is the scope of the
/// element's children.
pub fn check(
    attrs: &mut NodeAttrs,
    scope: &Scope,
    view: &dyn SchemaView,
    ctx: &AttrCtx,
) -> Result<Checked, Vec<ProfileError>> {
    let mut c = C {
        scope: scope.clone(),
        view,
        ctx,
        out: Checked::default(),
        errs: Vec::new(),
    };
    c.run(attrs, scope);
    c.out.paths.sort();
    c.out.paths.dedup();
    c.out.intents.sort();
    c.out.intents.dedup();
    c.out.handlers.sort();
    c.out.handlers.dedup();
    c.out.scope = c.scope;
    if c.errs.is_empty() {
        Ok(c.out)
    } else {
        Err(c.errs)
    }
}

fn scalar(t: &TypeSig) -> bool {
    !matches!(t, TypeSig::List(_) | TypeSig::Struct(_))
}

/// Whether a value of type `ty` may be stored in or passed as `param`.
fn assignable(ty: &Ty, param: &TypeSig) -> bool {
    let (nullable_ok, want) = match param {
        TypeSig::Opt(inner) => (true, inner.as_ref()),
        other => (false, other),
    };
    if ty.nullable && !nullable_ok {
        return false;
    }
    if ty.alts.is_empty() {
        return nullable_ok;
    }
    ty.alts.iter().all(|a| match (a, want) {
        (TypeSig::Int, TypeSig::Int | TypeSig::Float) | (TypeSig::Float, TypeSig::Float) => true,
        (TypeSig::Bool, TypeSig::Bool) | (TypeSig::Color, TypeSig::Color) => true,
        (TypeSig::Str, TypeSig::Str | TypeSig::Text) | (TypeSig::Text, TypeSig::Text) => true,
        (TypeSig::Asset, TypeSig::Asset) => true,
        (TypeSig::Struct(x), TypeSig::Struct(y)) => x == y,
        (TypeSig::Key(x), TypeSig::Key(y)) => x == y,
        (TypeSig::Enum(x), TypeSig::Enum(y)) => x.iter().all(|v| y.contains(v)),
        (TypeSig::Enum(_), TypeSig::Str) => true,
        _ => false,
    })
}

impl C<'_> {
    fn push(&mut self, code: &'static str, attr: &str, message: impl Into<String>) {
        self.errs.push(ProfileError::new(code, attr, message));
    }

    fn expr_err(&mut self, attr: &str, e: ExprError) {
        self.errs.push(ProfileError::from_expr(attr, e, 0));
    }

    fn analyze(&mut self, attr: &str, e: &Expr, scope: &Scope) -> Option<Analysis> {
        match analyze(e, scope, self.view) {
            Ok(a) => {
                self.out.paths.extend(a.paths.iter().cloned());
                Some(a)
            }
            Err(err) => {
                self.expr_err(attr, err);
                None
            }
        }
    }

    fn run(&mut self, attrs: &mut NodeAttrs, outer: &Scope) {
        // The loop first: everything else on the node is evaluated once per item.
        let mut inner = outer.clone();
        if let Some(f) = &mut attrs.for_ {
            if let Some(list) = self.analyze("data-bind-for", &f.list, outer) {
                match outer.for_scope(&f.item, f.index.as_deref(), &list, self.view) {
                    Ok(s) => inner = s,
                    Err(e) => self.expr_err("data-bind-for", e),
                }
                self.loop_key(f, &list, &inner);
            }
            if let Some(sort) = &f.sort
                && let Some(a) = self.analyze("data-bind-sort", sort, &inner)
                && !a
                    .ty
                    .alts
                    .iter()
                    .all(|t| matches!(t, TypeSig::Int | TypeSig::Float | TypeSig::Str))
            {
                self.push(
                    codes::TYPE,
                    "data-bind-sort",
                    "the sort key is a number or a string",
                );
            }
            if let Some(e) = &f.sort_desc {
                self.analyze("data-bind-sort-desc", e, outer);
            }
            if let Some(e) = &f.filter {
                self.analyze("data-bind-filter", e, &inner);
            }
        }
        // Under a condition the paths it proves not null are not nullable (on this node and its children).
        if let Some(c) = &attrs.cond {
            inner = inner.narrowed_by(c);
        }
        self.scope = inner.clone();
        let inner = &inner;

        if let Some(e) = &attrs.cond {
            self.analyze("data-bind-if", e, outer);
        }
        if let Some(t) = &attrs.value {
            self.template("data-bind-value", t, inner);
        }
        for ct in &attrs.class_toggle {
            self.analyze("data-bind-class-toggle", &ct.cond, inner);
        }
        for sb in &attrs.style {
            self.template("data-bind-style", &sb.tpl, inner);
            let def = Profile::v1().def(sb.prop);
            if def.kind == ValueKind::Color
                && let [TplSeg::Expr(e)] = sb.tpl.segs.as_slice()
                && let Some(a) = self.analyze("data-bind-style", e, inner)
                && !a
                    .ty
                    .alts
                    .iter()
                    .all(|t| matches!(t, TypeSig::Color | TypeSig::Str))
            {
                self.push(
                    codes::TYPE,
                    "data-bind-style",
                    "a color property takes a color or a string",
                );
            }
        }
        if let Some(e) = &attrs.src
            && let Some(a) = self.analyze("data-bind-src", e, inner)
            && !a.ty.alts.iter().all(|t| matches!(t, TypeSig::Asset))
        {
            self.push(
                codes::TYPE,
                "data-bind-src",
                "the picture is an asset reference",
            );
        }
        if let Some(e) = &attrs.disabled {
            self.analyze("data-bind-disabled", e, inner);
        }
        for e in &attrs.l10n_args {
            if let Some(a) = self.analyze("data-l10n-args", e, inner)
                && !a.ty.alts.iter().all(scalar)
            {
                self.push(
                    codes::TYPE,
                    "data-l10n-args",
                    "an argument is a single value",
                );
            }
        }
        if let Some((kind, e)) = &attrs.anchor {
            self.anchor(*kind, e, inner);
        }
        if let Some(c) = &attrs.control
            && let Some(e) = &c.options
            && let Some(a) = self.analyze("data-options", e, inner)
            && !a.ty.alts.iter().all(|t| matches!(t, TypeSig::List(_)))
        {
            self.push(
                expr::codes::NOT_LIST,
                "data-options",
                "the options are a list",
            );
        }
        if let Some(model) = attrs.model.clone() {
            self.model(&model, attrs.control.as_ref().map(|c| c.kind));
        }
        for ev in &mut attrs.events {
            for a in &mut ev.actions {
                self.action(a, inner);
            }
        }
    }

    fn template(&mut self, attr: &str, t: &expr::Template, scope: &Scope) {
        match check_template(t, scope, self.view) {
            Ok(paths) => self.out.paths.extend(paths),
            Err(e) => self.expr_err(attr, e),
        }
    }

    fn loop_key(&mut self, f: &mut super::ForSpec, list: &Analysis, inner: &Scope) {
        if let Some(key) = &f.key {
            if let Some(a) = self.analyze("data-bind-key", key, inner)
                && (a.ty.alts.is_empty()
                    || !a.ty.alts.iter().all(|t| {
                        matches!(
                            t,
                            TypeSig::Int | TypeSig::Str | TypeSig::Key(_) | TypeSig::Enum(_)
                        )
                    }))
            {
                self.push(
                    codes::TYPE,
                    "data-bind-key",
                    "a key is an integer, a string or a list key",
                );
            }
            return;
        }
        let key = match list.ty.alts.as_slice() {
            [sig @ TypeSig::List(_)] => self.view.list_key(sig).map(str::to_string),
            _ => None,
        };
        match key {
            Some(field) => f.key = Some(Expr::Path(vec![f.item.clone(), field])),
            None => self.push(
                codes::NO_KEY,
                "data-bind-for",
                "the list has no key in the schema; add data-bind-key",
            ),
        }
    }

    fn anchor(&mut self, kind: AnchorKind, e: &Expr, scope: &Scope) {
        let Some(a) = self.analyze("data-anchor", e, scope) else {
            return;
        };
        let ok = a.ty.alts.iter().all(|t| match t {
            TypeSig::Int => true,
            TypeSig::Key(k) => kind == AnchorKind::Plot || k == kind.name(),
            _ => false,
        });
        if !ok || a.ty.alts.is_empty() {
            self.push(
                codes::TYPE,
                "data-anchor",
                format!("a {} anchor needs a {} key", kind.name(), kind.name()),
            );
        }
    }

    fn model(&mut self, path: &str, control: Option<ControlKind>) {
        let Some(ty) = self.scope.state.get(path).cloned() else {
            self.push(
                expr::codes::UNKNOWN_PATH,
                "data-bind-model",
                format!("`state.{path}` is not in the skin state"),
            );
            return;
        };
        let ok = match control {
            Some(ControlKind::Checkbox | ControlKind::Toggle) => ty == TypeSig::Bool,
            Some(ControlKind::Slider) => matches!(ty, TypeSig::Int | TypeSig::Float),
            Some(ControlKind::Radio | ControlKind::Select) => scalar(&ty),
            None => matches!(ty, TypeSig::Str),
        };
        if !ok {
            self.push(
                codes::TYPE,
                "data-bind-model",
                format!("the state value is {ty}, which this control cannot edit"),
            );
        }
    }

    fn target(&mut self, path: &str) -> Option<TypeSig> {
        if let Some(rest) = path.strip_prefix("state.") {
            return match self.scope.state.get(rest) {
                Some(t) => Some(t.clone()),
                None => {
                    self.push(
                        expr::codes::UNKNOWN_PATH,
                        "",
                        format!("`{path}` is not in the skin state"),
                    );
                    None
                }
            };
        }
        let Some(ty) = resolve_path(self.view, path) else {
            self.push(
                expr::codes::UNKNOWN_PATH,
                "",
                format!("`{path}` is not a model path"),
            );
            return None;
        };
        if !self.view.js_writable(path) {
            self.push(
                codes::NOT_WRITABLE,
                "",
                format!("`{path}` is read-only for markup"),
            );
            return None;
        }
        Some(ty)
    }

    fn action(&mut self, a: &mut Action, scope: &Scope) {
        match a {
            Action::Stop => {}
            Action::Toggle { path } => {
                if let Some(t) = self.target(path)
                    && !matches!(&t, TypeSig::Bool)
                {
                    self.push(
                        codes::TYPE,
                        "",
                        format!("`toggle` flips a bool, `{path}` is {t}"),
                    );
                }
            }
            Action::Set { path, value } => {
                let target = self.target(path);
                if let Some(an) = self.analyze("", value, scope)
                    && let Some(t) = target
                {
                    self.assignment(&an.ty, &t, value);
                }
            }
            Action::Call { name, args } => {
                for arg in args.iter() {
                    self.analyze("", &arg.value, scope);
                }
                self.out.handlers.push(name.clone());
            }
            Action::Intent { name, args } => self.intent(name, args, scope),
        }
    }

    fn assignment(&mut self, ty: &Ty, target: &TypeSig, value: &Expr) {
        if let (Some(values), Expr::Str(s)) = (enum_values(target), value) {
            if !values.contains(s) {
                self.push(
                    expr::codes::ENUM_VALUE,
                    "",
                    format!("'{s}' is not one of {}", values.join(", ")),
                );
            }
            return;
        }
        if !assignable(ty, target) {
            self.push(
                codes::TYPE,
                "",
                format!("a value of type {ty} cannot be stored as {target}"),
            );
        }
    }

    fn intent(&mut self, name: &str, args: &mut [Arg], scope: &Scope) {
        if let Some(rest) = name.strip_prefix("mods.") {
            let own = self.ctx.mod_id.as_deref();
            let id = rest.split('.').next().unwrap_or("");
            if own != Some(id) || !rest.contains('.') {
                self.push(
                    codes::MOD_INTENT,
                    "",
                    "a mod fragment may send only `mods.<its own id>.<name>`",
                );
                return;
            }
            for arg in args.iter() {
                self.analyze("", &arg.value, scope);
            }
            return;
        }
        let Some(decl) = self.view.intent(name) else {
            self.push(
                codes::UNKNOWN_INTENT,
                "",
                format!("`{name}` is not in the intent catalogue"),
            );
            return;
        };
        let positional = args.first().is_some_and(|a| a.name.is_none());
        let mut ordered: Vec<Option<usize>> = vec![None; decl.len()];
        if positional {
            if args.len() > decl.len() {
                self.push(
                    codes::INTENT_ARGS,
                    "",
                    format!("`{name}` takes {} arguments", decl.len()),
                );
                return;
            }
            for (i, slot) in ordered.iter_mut().enumerate().take(args.len()) {
                *slot = Some(i);
            }
        } else {
            for (i, arg) in args.iter().enumerate() {
                let Some(at) = decl.iter().position(|(n, _)| Some(n) == arg.name.as_ref()) else {
                    self.push(
                        codes::INTENT_ARGS,
                        "",
                        format!(
                            "`{name}` has no argument `{}`",
                            arg.name.clone().unwrap_or_default()
                        ),
                    );
                    return;
                };
                ordered[at] = Some(i);
            }
        }
        for (slot, (pname, ptype)) in ordered.iter().zip(&decl) {
            if slot.is_none() && !matches!(ptype, TypeSig::Opt(_)) {
                self.push(
                    codes::INTENT_ARGS,
                    "",
                    format!("`{name}` needs the argument `{pname}`"),
                );
                return;
            }
        }
        let mut ok = true;
        for (slot, (pname, ptype)) in ordered.iter().zip(&decl) {
            let Some(i) = *slot else { continue };
            let value = &args[i].value;
            let Some(an) = self.analyze("", value, scope) else {
                ok = false;
                continue;
            };
            let before = self.errs.len();
            // A key argument may be a key that can be null (read through an optional record): the intent's
            // route drops an intent whose key is missing, so the markup needs no guard for it. A plain `null`
            // stays an error.
            let optional;
            let ptype = if matches!(ptype, TypeSig::Key(_)) && !an.ty.alts.is_empty() {
                optional = TypeSig::Opt(Box::new(ptype.clone()));
                &optional
            } else {
                ptype
            };
            self.assignment(&an.ty, ptype, value);
            if self.errs.len() > before {
                ok = false;
                if let Some(e) = self.errs.last_mut() {
                    e.message = format!("argument `{pname}` of `{name}`: {}", e.message);
                }
            }
        }
        if !ok {
            return;
        }
        let mut named: Vec<Arg> = Vec::new();
        for (slot, (pname, _)) in ordered.iter().zip(&decl) {
            if let Some(i) = *slot {
                named.push(Arg {
                    name: Some(pname.clone()),
                    value: args[i].value.clone(),
                });
            }
        }
        for (dst, src) in args.iter_mut().zip(named.iter()) {
            *dst = src.clone();
        }
        self.out.intents.push((
            name.to_string(),
            decl.iter().map(|(_, t)| t.clone()).collect(),
        ));
    }
}

fn enum_values(t: &TypeSig) -> Option<&Vec<String>> {
    match t {
        TypeSig::Enum(v) => Some(v),
        TypeSig::Opt(inner) => enum_values(inner),
        _ => None,
    }
}
