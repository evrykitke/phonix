//! Which currencies a workspace deals in, and what it wants printed.
//!
//! # The row is the selection, not the currency
//!
//! Names and minor units come from [`Currency`], which is compiled into both
//! the server and the browser bundle. This type says only *which* codes a
//! workspace uses and what symbol it wants beside them - so a currency list is
//! never a second copy of ISO 4217 with its own answer to "how many decimal
//! places does the yen have".
//!
//! # Why the symbol survived the cut
//!
//! Because it genuinely is the organization's choice. `$` is a dozen different
//! currencies and which one it means depends entirely on who is reading, so a
//! workspace invoicing in two of them has an opinion that ISO does not.
//!
//! Here rather than in the repository so that it can cross the wire: the
//! settings screen that edits this list runs in the browser.

use serde::{Deserialize, Serialize};

use crate::locale::Currency;

/// One currency a workspace has switched on, with its own display choices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCurrency {
    pub currency: Currency,
    /// A disabled currency stays resolvable but leaves the pickers.
    ///
    /// There is no delete. Rates and posted documents still have to resolve,
    /// and a foreign-key error naming `exchange_rates` is not a useful answer
    /// to somebody tidying a settings screen.
    pub is_enabled: bool,
    /// What to print instead of the code, when the organization has an
    /// opinion. `None` means the code.
    pub symbol: Option<String>,
}

impl WorkspaceCurrency {
    /// What to print beside an amount: the chosen symbol, or the code.
    pub fn display(&self) -> &str {
        self.symbol
            .as_deref()
            .filter(|symbol| !symbol.trim().is_empty())
            .unwrap_or_else(|| self.currency.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(symbol: Option<&str>) -> WorkspaceCurrency {
        WorkspaceCurrency {
            currency: Currency::parse("USD").expect("a real currency"),
            is_enabled: true,
            symbol: symbol.map(str::to_owned),
        }
    }

    #[test]
    fn a_workspace_with_no_opinion_prints_the_code() {
        assert_eq!(selection(None).display(), "USD");
        // Blank is not an opinion either - it is a field somebody cleared.
        assert_eq!(selection(Some("  ")).display(), "USD");
    }

    #[test]
    fn a_workspace_with_an_opinion_gets_it() {
        assert_eq!(selection(Some("$")).display(), "$");
    }
}
