//! See https://immunant.com/blog/2023/03/lifting/ for more information on
//! pointer derivation graph (PDG) matching.

use crate::monad::ast::Pass;
use crate::MonadicAst;
use quote::quote;
use std::collections::{HashMap, HashSet};
use syn::visit::Visit;
use syn::visit_mut::VisitMut;
use syn::{
    Expr, ExprAssign, ExprMethodCall, ExprPath, ExprUnary, File, FnArg, Ident, Local, Pat,
    PatIdent, PatType, Type, TypePtr, UnOp,
};

/// Represents a permission that a raw pointer *p will need at the point in the
/// program p is defined and used.
#[derive(Copy, Clone, Debug, Hash, Eq, Ord, PartialEq, PartialOrd)]
enum PointerAccess {
    Write,     // The program writes to the pointee.
    Unique,    // The pointer is the only way to access the given memory location.
    Free,      // The pointer will eventually be passed to free.
    OffsetAdd, // We'll add an offset to the pointer, e.g. array element access.
    OffsetSub, // We'll subtract an offset to the pointer.
}

#[derive(Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum RustPointerType {
    ImmutableReference, // &T
    MutableReference,   // &mut T
    CellReference,      // &Cell<T>
    UniquePointer,      // Box<T>
    ImmutableSlice,     // &[T]
    MutableSlice,       // &mut [T]
    UniqueSlicePointer, // Box<[T]>
    Undefined,          // ...for unsupported combinations
}

static ACCESSES: &[PointerAccess] = &[
    PointerAccess::Write,
    PointerAccess::Unique,
    PointerAccess::Free,
    PointerAccess::OffsetAdd,
    PointerAccess::OffsetSub,
];

impl PointerAccess {
    /// Returns the Rust safe pointer type corresponding to the given pointer access
    /// permissions, if any exists, and RustPointerType::Undefined otherwise.
    ///
    /// The permissions to type mapping is determined by the following table:
    /// Write - Unique - Free - Offset  |  Resulting Type
    ///                                 |      &T
    ///   X       X                     |      &mut T
    ///   X                             |      &Cell<T>
    ///           X       X             |      Box<T>
    ///                           X     |      &[T]
    ///   X       X               X     |      &mut [T]
    ///           X       X       X     |      Box<[T]>
    fn determine_rust_type(permissions: &[PointerAccess]) -> RustPointerType {
        let [has_write, has_unique, has_free, has_offset_add, has_offset_sub]: [bool; 5] = ACCESSES
            .iter()
            .map(|access_type| permissions.contains(access_type))
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        match (
            has_write,
            has_unique,
            has_free,
            has_offset_add,
            has_offset_sub,
        ) {
            // &T
            (false, false, false, false, false) => RustPointerType::ImmutableReference,
            // Write + Unique -> &mut T
            (true, true, false, false, false) => RustPointerType::MutableReference,
            // Write -> &Cell<T>
            (true, false, false, false, false) => RustPointerType::CellReference,
            // Unique + Free -> Box<T>
            (false, true, true, false, false) => RustPointerType::UniquePointer,
            // Offset -> &[T]
            (false, false, false, true, true)
            | (false, false, false, true, false)
            | (false, false, false, false, true) => RustPointerType::ImmutableSlice,
            // Write + Unique + Offset -> &mut [T]
            (true, true, false, true, true)
            | (true, true, false, true, false)
            | (true, true, false, false, true) => RustPointerType::MutableSlice,
            // Unique + Free + Offset -> Box<T>
            (false, true, true, true, true)
            | (false, true, true, true, false)
            | (false, true, true, false, true) => RustPointerType::UniqueSlicePointer,
            _ => RustPointerType::Undefined,
        }
    }
}

#[derive(Default)]
enum TypeMappingStateMachine {
    /// Still identifying usages of raw pointers, or the process of mapping them
    /// to their appropriate Rust safe reference type hasn't started yet.
    #[default]
    Uninitialized,
    /// Currently in the process of mapping identifiers to their appropriate Rust
    /// safe reference types.
    Computing(HashMap<Ident, RustPointerType>),
    /// All raw pointer identifiers have been mapped to their appropriate Rust
    /// safe reference type.
    Initialized(HashMap<Ident, RustPointerType>),
}

#[derive(Default)]
pub struct RawPointerSanitizer {
    /// Keeps track of pointer variables and their access permissions.
    pointers: HashMap<Ident, (TypePtr, HashSet<PointerAccess>)>,
    /// Mapping between the pointer variables and their memory safe equivalent types.
    types: TypeMappingStateMachine,
}

impl RawPointerSanitizer {
    fn record_if_pointer(&mut self, pat: &Pat, ty: &Type) {
        match (pat, ty) {
            (
                Pat::Ident(PatIdent {
                    mutability: _,
                    ident,
                    ..
                }),
                Type::Ptr(pointer),
            ) => {
                self.pointers
                    .insert(ident.clone(), (pointer.clone(), HashSet::new()));
            }
            _ => {}
        }
    }

    fn identify_raw_pointer_args(&mut self, ast: &mut File) {
        self.visit_file(ast);

        // Advance state from 'Uninitialized' to 'Computing'
        match self.types {
            TypeMappingStateMachine::Uninitialized => {
                self.types = TypeMappingStateMachine::Computing(HashMap::new())
            }
            _ => panic!("Must be in Uninitialized state"),
        }
    }

    fn compute_equivalent_safe_types(&mut self) {
        // TODO: compute type equivalents

        // Advance state from `Computing` to `Initialized`.
        let old_state = std::mem::replace(&mut self.types, TypeMappingStateMachine::Uninitialized);
        match old_state {
            TypeMappingStateMachine::Computing(map) => {
                self.types = TypeMappingStateMachine::Initialized(map)
            }
            _ => {
                let _ = std::mem::replace(&mut self.types, old_state);
                panic!("Must be in Computing state")
            }
        }
    }
}

impl Visit<'_> for RawPointerSanitizer {
    /// Inspects a function argument and adds it to the `pointers` map if it is a
    /// raw pointer type.
    fn visit_fn_arg(&mut self, arg: &FnArg) {
        if let FnArg::Typed(PatType { pat, ty, .. }) = arg {
            self.record_if_pointer(&**pat, &**ty)
        }
        syn::visit::visit_fn_arg(self, arg)
    }

    /// Inspects a local variable declaration and adds it to the `pointers` map if it
    /// is a raw pointer type declaration.
    fn visit_local(&mut self, assignment: &Local) {
        if let Pat::Type(PatType { pat, ty, .. }) = &assignment.pat {
            self.record_if_pointer(&**pat, &**ty)
        }
        syn::visit::visit_local(self, assignment)
    }

    fn visit_expr_assign(&mut self, i: &'_ ExprAssign) {
        fn get_pointer_accesses_mut<'a, 'b>(
            receiver: &'a Box<Expr>,
            pointers: &'b mut HashMap<Ident, (TypePtr, HashSet<PointerAccess>)>,
        ) -> Option<&'b mut HashSet<PointerAccess>> {
            match receiver.as_ref() {
                Expr::Path(ExprPath { qself, path, .. }) => {
                    if qself.is_some() {
                        return None;
                    }
                    println!("herrrr");
                    let ident = &path.segments.last().unwrap().ident;
                    pointers
                        .get_mut(ident)
                        .map_or_else(|| None, |(_, map)| Some(map))
                }
                _ => None,
            }
        }

        let ExprAssign { left, right, .. } = i;

        // lvalue pointer access.
        // * (p (.offset()))
        if let Expr::Unary(ExprUnary { op, expr, .. }) = left.as_ref() {
            println!("unary");
            if let UnOp::Deref(_) = op {
                println!("deref");
                if let Expr::MethodCall(ExprMethodCall {
                    method, receiver, ..
                }) = expr.as_ref()
                {
                    println!("method call {}", quote! { #method });
                    if let Some(access_set) = get_pointer_accesses_mut(receiver, &mut self.pointers)
                    {
                        // TODO(eyoon): If method == offset: pointers.get_mut(ident) access insert offset
                        access_set.insert(PointerAccess::Write);
                        println!("Lvalue access {:?}", access_set);
                    }
                }
            }
        }
    }

    /// Inspects a method call, updating the pointer accesses mapping if the call is a
    /// raw pointer access.
    fn visit_expr_method_call(&mut self, expr: &ExprMethodCall) {
        let ExprMethodCall {
            method, receiver, ..
        } = expr;
        // debug stuff ahaha
        println!("Visit - {}", quote! { #expr }.to_string());
        println!(" -- {}", quote! { #method }.to_string());
        println!(" -! {}", quote! { #receiver }.to_string());

        // TODO(eyoon): Need to correctly handle variable shadowing.
        //              (Or can we assume generated code won't have shadowed vars?)
        if let Expr::Path(ExprPath { attrs, qself, path }) = receiver.as_ref() {
            if qself.is_none() {
                let ident = &path.segments.last().unwrap().ident;
                if self.pointers.contains_key(ident) {
                    println!(" --- raw pointer ident: {}", quote! { #ident}.to_string())
                }
            }
        }

        syn::visit::visit_expr_method_call(self, expr)
    }
}

impl VisitMut for RawPointerSanitizer {
    // TODO
}

impl Pass for RawPointerSanitizer {
    fn bind(&mut self, mut monad: MonadicAst) -> MonadicAst {
        self.identify_raw_pointer_args(&mut monad.ast);
        self.compute_equivalent_safe_types();

        // TODO - Replaces the types of the raw pointer variables with their memory safe Rust
        //      - equivalents, computed from their access permissions. Updates the accesses of
        //      - the updated variables, as necessary.
        self.visit_file_mut(&mut monad.ast);

        monad
    }
}
