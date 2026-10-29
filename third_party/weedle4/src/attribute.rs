use crate::argument::ArgumentList;
use crate::common::{Bracketed, Identifier, Parenthesized, Punctuated};
use crate::literal::*;

/// Parses a list of attributes. Ex: `[ attribute1, attribute2 ]`
pub type ExtendedAttributeList<'a> = Bracketed<Punctuated<ExtendedAttribute<'a>, term!(,)>>;

/// Matches comma separated identifier list
pub type IdentifierList<'a> = Punctuated<Identifier<'a>, term!(,)>;

/// Whether the list carries `[name]` with no arguments.
pub fn has_no_args(list: &ExtendedAttributeList<'_>, name: &str) -> bool {
    list.body.list.iter().any(|attribute| match attribute {
        ExtendedAttribute::NoArgs(item) => item.0 .0 == name,
        _ => false,
    })
}

/// Whether `[Exposed]` names `target`, including the `*` wildcard.
pub fn exposed_in(list: &ExtendedAttributeList<'_>, target: &str) -> bool {
    list.body.list.iter().any(|attribute| match attribute {
        ExtendedAttribute::WildCard(item) if item.identifier.0 == "Exposed" => true,
        ExtendedAttribute::Ident(item) if item.lhs_identifier.0 == "Exposed" => {
            item.rhs.0 == target
        }
        ExtendedAttribute::IdentList(item) if item.identifier.0 == "Exposed" => {
            item.list.body.list.iter().any(|item| item.0 == target)
        }
        _ => false,
    })
}

ast_types! {
    /// Parses on of the forms of attribute
    enum ExtendedAttribute<'a> {
        /// Parses an argument list. Ex: `Constructor((double x, double y))`
        ///
        /// (( )) means ( ) chars
        ArgList(struct ExtendedAttributeArgList<'a> {
            identifier: Identifier<'a>,
            args: Parenthesized<ArgumentList<'a>>,
        }),
        /// Parses a named argument list. Ex: `NamedConstructor=Image((DOMString src))`
        ///
        /// (( )) means ( ) chars
        NamedArgList(struct ExtendedAttributeNamedArgList<'a> {
            lhs_identifier: Identifier<'a>,
            assign: term!(=),
            rhs_identifier: Identifier<'a>,
            args: Parenthesized<ArgumentList<'a>>,

        }),
        /// Parses an identifier list. Ex: `Exposed=((Window,Worker))`
        ///
        /// (( )) means ( ) chars
        IdentList(struct ExtendedAttributeIdentList<'a> {
            identifier: Identifier<'a>,
            assign: term!(=),
            list: Parenthesized<IdentifierList<'a>>,
        }),
        /// Parses a wildcard. Ex: `[Exposed=*]`
        WildCard(struct ExtendedAttributeWildcard<'a> {
            identifier: Identifier<'a>,
            assign: term!(=),
            list: term!(*),
        }),
        #[derive(Copy)]
        Decimal(struct ExtendedAttributeDecimal<'a> {
            lhs_identifier: Identifier<'a>,
            assign: term!(=),
            rhs: FloatValueLit<'a>,
        }),
        /// Parses an attribute with an identifier. Ex: `PutForwards=name`
        /// <https://webidl.spec.whatwg.org/#prod-ExtendedAttributeIdent>
        #[derive(Copy)]
        Ident(struct ExtendedAttributeIdent<'a> {
            lhs_identifier: Identifier<'a>,
            assign: term!(=),
            rhs: Identifier<'a>,
        }),
        #[derive(Copy)]
        String(struct ExtendedAttributeString<'a> {
            lhs_identifier: Identifier<'a>,
            assign: term!(=),
            rhs: StringLit<'a>,
        }),
        /// Parses a plain attribute. Ex: `Replaceable`
        #[derive(Copy)]
        NoArgs(struct ExtendedAttributeNoArgs<'a>(
            Identifier<'a>,
        )),
    }

    /// Parses `stringifier|static`
    #[derive(Copy)]
    enum IdentifierOrString<'a> {
        Identifier(Identifier<'a>),
        String(StringLit<'a>),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::Parse;

    test!(should_parse_attribute_no_args { "Replaceable" =>
        "";
        ExtendedAttributeNoArgs => ExtendedAttributeNoArgs(Identifier("Replaceable"))
    });

    test!(should_parse_attribute_arg_list { "Constructor(double x, double y)" =>
        "";
        ExtendedAttributeArgList;
        identifier.0 == "Constructor";
        args.body.list.len() == 2;
    });

    test!(should_parse_attribute_ident { "PutForwards=name" =>
        "";
        ExtendedAttributeIdent;
        lhs_identifier.0 == "PutForwards";
        rhs == Identifier("name");
    });

    test!(should_parse_attribute_decimal { "PutForwards=1.0" =>
        "";
        ExtendedAttributeDecimal;
        lhs_identifier.0 == "PutForwards";
        rhs == FloatValueLit("1.0");
    });

    test!(should_parse_ident_list { "Exposed=(Window,Worker)" =>
        "";
        ExtendedAttributeIdentList;
        identifier.0 == "Exposed";
        list.body.list.len() == 2;
    });

    test!(should_parse_wildcard { "Exposed=*" =>
        "";
        ExtendedAttributeWildcard;
        identifier.0 == "Exposed";
    });

    test!(should_parse_named_arg_list { "NamedConstructor=Image(DOMString src)" =>
        "";
        ExtendedAttributeNamedArgList;
        lhs_identifier.0 == "NamedConstructor";
        rhs_identifier.0 == "Image";
        args.body.list.len() == 1;
    });
}
