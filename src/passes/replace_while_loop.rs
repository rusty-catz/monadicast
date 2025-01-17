use crate::monad::ast::Pass;
use crate::MonadicAst;
use proc_macro2;
use std::collections::HashMap;
use syn::{visit::Visit, visit_mut::VisitMut, Expr, ExprLit, ExprWhile, Lit, LitInt, Pat, Stmt};

#[derive(Default)]
pub struct WhileLoopReplacer {
    loop_vars: HashMap<String, i32>,
}

impl WhileLoopReplacer {
    fn record_if_whileloop(&mut self, wloop: &ExprWhile) {
        println!("{:?}", wloop);
        println!("Found a while loop");
    }

    // Helper function to check if a statement is incrementing a specific variable
    fn is_increment_stmt(&self, stmt: &Stmt, var_name: &str) -> bool {
        println!("\n\nFOUND STMT {:?}\n\n", stmt);
        match stmt {
            // Check for assignment expressions (i = i + 1)
            Stmt::Expr(Expr::Assign(assign), _) => {
                if let Expr::Path(path) = &*assign.left {
                    let left_var = path.path.segments[0].ident.to_string();
                    if left_var == var_name {
                        // Check if right side is an increment
                        if let Expr::Binary(binary) = &*assign.right {
                            if let (Expr::Path(left_path), Expr::Lit(right_lit)) =
                                (&*binary.left, &*binary.right)
                            {
                                return left_path.path.segments[0].ident.to_string() == var_name;
                            }
                        }
                    }
                }
                false
            }

            Stmt::Expr(Expr::Binary(binary), _) => {
                // check if we have a PlusEq with the target variable
                if let (Expr::Path(left), Expr::Lit(right)) = (&*binary.left, &*binary.right) {
                    if left.path.segments[0].ident.to_string() == var_name {
                        if let Lit::Int(int_lit) = &right.lit {
                            return int_lit.base10_parse::<i32>().unwrap() == 1;
                        }
                    }
                }
                false
            }
            // Check for expressions with semicolons
            _ => false,
        }
    }
}

impl Visit<'_> for WhileLoopReplacer {
    fn visit_expr_while(&mut self, whileloop: &ExprWhile) {
        println!("Found a while loop");
        self.record_if_whileloop(whileloop);
    }
}
impl VisitMut for WhileLoopReplacer {
    fn visit_stmt_mut(&mut self, stmt: &mut Stmt) {
        if let Stmt::Local(local) = stmt {
            let mut variable_name = String::new();
            if let Pat::Type(t) = &local.pat {
                if let Pat::Ident(ident) = &*t.pat {
                    variable_name = ident.ident.to_string();
                }
            }
            if let Some(local_init) = local.init.as_ref() {
                if let Expr::Cast(cast_expr) = &*local_init.expr {
                    if let Expr::Lit(lit) = &*cast_expr.expr {
                        if let ExprLit {
                            lit: Lit::Int(int_lit),
                            ..
                        } = lit
                        {
                            let int_lit = int_lit.base10_parse::<i32>().unwrap();
                            self.loop_vars.insert(variable_name.clone(), int_lit);
                        }
                    }
                }
            }
        }

        if let Stmt::Expr(Expr::While(while_loop), _) = stmt {
            if let Expr::Binary(cond) = &*while_loop.cond {
                // Get the left variable (iterator variable)
                let l_var = if let Expr::Path(left) = &*cond.left {
                    left.path.segments[0].ident.to_string()
                } else {
                    return;
                };

                if self.loop_vars.contains_key(&l_var) {
                    // Create the lower bound
                    let lower_bound: syn::Expr = if self.loop_vars.contains_key(&l_var) {
                        let value = self.loop_vars.get(&l_var).unwrap();
                        syn::parse_str::<syn::Expr>(&value.to_string()).unwrap()
                    } else {
                        let ident = syn::Ident::new(&l_var, proc_macro2::Span::call_site());
                        syn::parse_quote!(#ident)
                    };

                    // Handle both variable and literal upper bounds
                    let upper_bound: syn::Expr = match &*cond.right {
                        Expr::Path(right) => {
                            let r_var = right.path.segments[0].ident.to_string();
                            if self.loop_vars.contains_key(&r_var) {
                                let value: &i32 = self.loop_vars.get(&r_var).unwrap();
                                syn::parse_str::<syn::Expr>(&value.to_string()).unwrap()
                            } else {
                                let ident: syn::Ident =
                                    syn::Ident::new(&r_var, proc_macro2::Span::call_site());
                                syn::parse_quote!(#ident)
                            }
                        }
                        Expr::Lit(lit) => {
                            if let Lit::Int(int_lit) = &lit.lit {
                                syn::parse_str::<syn::Expr>(&int_lit.to_string()).unwrap()
                            } else {
                                return;
                            }
                        }
                        _ => return,
                    };

                    let iter_var: syn::Ident =
                        syn::Ident::new(&l_var, proc_macro2::Span::call_site());

                    println!("HASHMAP: {:?}", self.loop_vars);

                    // Create the appropriate range expression based on the operator
                    let range: syn::Expr = match &cond.op {
                        syn::BinOp::Lt(_) => syn::parse_quote!(#lower_bound..#upper_bound),
                        syn::BinOp::Le(_) => syn::parse_quote!(#lower_bound..=#upper_bound),
                        _ => return, // Skip other operators
                    };

                    let filtered_stmts: Vec<Stmt> = while_loop
                        .body
                        .stmts
                        .iter()
                        .filter(|stmt| !self.is_increment_stmt(stmt, &l_var))
                        .copied()
                        .collect();

                    // Create a new block with the filtered statements
                    let new_body: syn::Block = syn::parse_quote! {{
                        #(#filtered_stmts)*
                    }};

                    // Create the for loop with the filtered body
                    let for_loop: syn::Expr = syn::parse_quote! {
                        for #iter_var in #range #new_body
                    };

                    // Replace the while loop with the for loop
                    *stmt = Stmt::Expr(for_loop, None);
                }
            }

            // Continue visiting other statements normally
            syn::visit_mut::visit_stmt_mut(self, stmt);
        }
    }
}

impl Pass for WhileLoopReplacer {
    fn bind(&mut self, mut monad: MonadicAst) -> MonadicAst {
        self.visit_file_mut(&mut monad.ast);
        monad
    }
}
