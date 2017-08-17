// Copyright (c) 2017 King's College London
// created by the Software Development Team <http://soft-dev.org/>
//
// The Universal Permissive License (UPL), Version 1.0
//
// Subject to the condition set forth below, permission is hereby granted to any person obtaining a
// copy of this software, associated documentation and/or data (collectively the "Software"), free
// of charge and under any and all copyright rights in the Software, and any and all patent rights
// owned or freely licensable by each licensor hereunder covering either (i) the unmodified
// Software as contributed to or provided by such licensor, or (ii) the Larger Works (as defined
// below), to deal in both
//
// (a) the Software, and
// (b) any piece of software and/or hardware listed in the lrgrwrks.txt file
// if one is included with the Software (each a "Larger Work" to which the Software is contributed
// by such licensors),
//
// without restriction, including without limitation the rights to copy, create derivative works
// of, display, perform, and distribute the Software and make, use, sell, offer for sale, import,
// export, have made, and have sold the Software and the Larger Work(s), and to sublicense the
// foregoing rights on either these or other terms.
//
// This license is subject to the following condition: The above copyright notice and either this
// complete permission notice or at a minimum a reference to the UPL must be included in all copies
// or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
// BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
// NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
// DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

use std::convert::{TryFrom, TryInto};
use std::fmt::Debug;

use cfgrammar::Symbol;
use lrlex::Lexeme;
use lrtable::Action;

use parser::{Parser, ParseRepair, PStack, TStack};

const PARSE_AT_LEAST: usize = 3; // N in Corchuelo et al.
const PORTION_THRESHOLD: usize = 10; // N_t in Corchuelo et al.
const INSERT_THRESHOLD: usize = 4; // N_i in Corchuelo et al.
const DELETE_THRESHOLD: usize = 3; // N_d in Corchuelo et al.

pub(crate) fn recover<TokId: Clone + Copy + Debug + TryFrom<usize> + TryInto<usize> + PartialEq>
                     (parser: &Parser<TokId>, in_la_idx: usize, in_pstack: &mut PStack,
                      in_tstack: &mut TStack<TokId>)
                  -> Vec<(PStack, TStack<TokId>, usize, Vec<ParseRepair>)>
{
    // This paper implements the algorithm from "Repairing syntax errors in LR parsers" by
    // Rafael Corchuelo, Jose A. Perez, Antonio Ruiz, and Miguel Toro.

    let mut todo = vec![(in_la_idx, in_pstack.clone(), in_tstack.clone(), vec![])];
    let mut finished = vec![];
    let mut finished_score = None;
    let mut dummy_errors = vec![];
    while todo.len() > 0 {
        let cur = todo.remove(0);
        let la_idx = cur.0;
        let pstack = cur.1;
        let tstack = cur.2;
        let repairs: Vec<ParseRepair> = cur.3;
        if finished_score.is_some() && finished_score.unwrap() < score(&repairs) {
            continue;
        }

        // Insertion rule (ER1)
        match repairs.last() {
            Some(&ParseRepair::Delete) => {
                // In order to avoid adding both [Del, Ins x] and [Ins x, Del] (which are
                // equivalent), we follow Corcheulo et al.'s suggestion and never add an Ins after
                // a Del.
            },
            _ => {
                let num_inserts = repairs.iter()
                                         .filter(|r| if let ParseRepair::Insert{..} = **r {
                                                         true
                                                     } else {
                                                         false
                                                     })
                                         .count();
                if num_inserts <= INSERT_THRESHOLD {
                    for sym in parser.stable.state_actions(*pstack.last().unwrap()) {
                        if let Symbol::Term(term_idx) = sym {
                            if term_idx == parser.grm.eof_term_idx() {
                                continue;
                            }
                            let mut n_pstack = pstack.clone();
                            let mut n_tstack = tstack.clone();

                            // We make the artificially inserted lexeme appear to start at the same
                            // position as the real next lexeme, but have zero length (so that it's
                            // clear it's not really something the user created).
                            let (next_lexeme, _) = parser.next_lexeme(None, la_idx);
                            let new_lexeme = Lexeme::new(TokId::try_from(usize::from(term_idx))
                                                                        .ok()
                                                                        .unwrap(),
                                                         next_lexeme.start(), 0);
                            let new_la_idx = parser.lr(Some(new_lexeme), la_idx,
                                                     Some(la_idx + 1), &mut n_pstack,
                                                     &mut n_tstack, &mut dummy_errors);
                            debug_assert_eq!(dummy_errors.len(), 0);
                            if new_la_idx > la_idx {
                                debug_assert_eq!(new_la_idx, la_idx + 1);
                                let mut n_repairs = repairs.clone();
                                n_repairs.push(ParseRepair::Insert{term_idx});
                                todo.push((la_idx, n_pstack, n_tstack, n_repairs));
                            }
                        }
                    }
                }
            }
        }

        // Delete rule (ER2)
        if la_idx < parser.lexemes.len() {
            let num_deletes = repairs.iter()
                                     .filter(|r| if let ParseRepair::Delete = **r {
                                                     true
                                                 } else {
                                                     false
                                                 })
                                     .count();
            if num_deletes <= DELETE_THRESHOLD {
                let mut n_repairs = repairs.clone();
                n_repairs.push(ParseRepair::Delete);
                todo.push((la_idx + 1, pstack.clone(), tstack.clone(), n_repairs));
            }
        }

        // Forward move rule (ER3)
        //
        // Note the rule in Corchuelo et al. is confusing and, I think, wrong. It reads:
        //   (S, I) \rightarrow_{LR*} (S', I') \wedge (j = N \vee 0 < j < N
        //                                             \wedge f(q_r, t_{j + 1} \in {accept, error})
        // First I think the bracketing would be clearer if written as:
        //   j = N \vee (0 < j < N \wedge f(q_r, t_{j + 1} \in {accept, error})
        // And I think the condition should be:
        //   j = N \vee (0 <= j < N \wedge f(q_r, t_{j + 1} \in {accept, error})
        // because there's no reason that any symbols need to be shifted in order for an accept
        // (or, indeed an error) state to be reached.
        //
        // So the full rule should I think be:
        //   (S, I) \rightarrow_{LR*} (S', I')
        //   \wedge (j = N \vee (0 <= j < N \wedge f(q_r, t_{j + 1} \in {accept, error}))
        {
            let mut n_pstack = pstack.clone();
            let mut n_tstack = tstack.clone();
            let new_la_idx = parser.lr(None, la_idx, Some(la_idx + PARSE_AT_LEAST),
                                       &mut n_pstack, &mut n_tstack, &mut dummy_errors);
            debug_assert_eq!(dummy_errors.len(), 0);
            if new_la_idx < in_la_idx + PORTION_THRESHOLD {
                let mut n_repairs = repairs.clone();
                for _ in la_idx..new_la_idx {
                    n_repairs.push(ParseRepair::Shift);
                }

                // A repair is a "finisher" (i.e. can be considered complete and doesn't need to be
                // added to the todo list) if it's parsed at least N symbols or parsing ends in
                // an Accept action.
                let mut finisher = false;
                if new_la_idx == la_idx + PARSE_AT_LEAST {
                    finisher = true;
                } else {
                    debug_assert!(new_la_idx < la_idx + PARSE_AT_LEAST);
                    let st = *n_pstack.last().unwrap();
                    let (_, la_term) = parser.next_lexeme(None, new_la_idx);
                    match parser.stable.action(st, la_term) {
                        Some(Action::Accept) => finisher = true,
                        None => (),
                        _ => continue,
                    }
                }

                if finisher {
                    let s = score(&n_repairs);
                    if finished_score.is_none() || s < finished_score.unwrap() {
                        finished_score = Some(s);
                        finished.clear();
                    }
                    finished.push((n_pstack, n_tstack, new_la_idx, n_repairs));
                } else if new_la_idx > la_idx {
                    todo.push((new_la_idx, n_pstack, n_tstack, n_repairs));
                }
            }
        }
    }
    finished
}

fn score(repairs: &Vec<ParseRepair>) -> usize {
    let mut count = 0;
    for r in repairs {
        match *r {
            ParseRepair::Insert{..} | ParseRepair::Delete => {
                count += 1;
            },
            ParseRepair::Shift => ()
        }
    }
    count
}

#[cfg(test)]
mod test {
    use cfgrammar::yacc::YaccGrammar;
    use std::convert::TryFrom;
    use lrlex::Lexeme;
    use parser::ParseRepair;
    use parser::test::do_parse;

    fn pp_repairs(grm: &YaccGrammar, repairs: &Vec<ParseRepair>) -> String {
        let mut out = vec![];
        for &r in repairs {
            match r {
                ParseRepair::Insert{term_idx} =>
                    out.push(format!("Insert \"{}\"", grm.term_name(term_idx).unwrap())),
                ParseRepair::Delete =>
                    out.push(format!("Delete")),
                ParseRepair::Shift =>
                    out.push(format!("Delete"))
            }
        }
        out.join(", ")
    }

    fn check_repairs(grm: &YaccGrammar, repairs: &Vec<Vec<ParseRepair>>, expected: &[&str]) {
        for i in 0..repairs.len() {
            // First of all check that all the repairs are unique
            for j in i + 1..repairs.len() {
                assert_ne!(repairs[i], repairs[j]);
            }
            println!("{}", pp_repairs(&grm, &repairs[i]));
            expected.iter().find(|x| **x == pp_repairs(&grm, &repairs[i])).unwrap();
        }
    }

    #[test]
    fn corchuelo_example() {
        // The example from the Curchuelo paper
        let lexs = "%%
[(] OPEN_BRACKET
[)] CLOSE_BRACKET
[+] PLUS
n N
";
        let grms = "%start E
%%
E : 'N'
  | E 'PLUS' 'N'
  | 'OPEN_BRACKET' E 'CLOSE_BRACKET'
  ;
";

        let (grm, pr) = do_parse(&lexs, &grms, "(nn");
        let errs = pr.unwrap_err();
        assert_eq!(errs.len(), 1);
        let err_tok_id = u16::try_from(usize::from(grm.term_idx("N").unwrap())).ok().unwrap();
        assert_eq!(errs[0].lexeme(), &Lexeme::new(err_tok_id, 2, 1));
        assert_eq!(errs[0].repairs().len(), 3);
        check_repairs(&grm,
                      errs[0].repairs(),
                      &vec!["Insert \"CLOSE_BRACKET\", Insert \"PLUS\", Delete",
                            "Insert \"CLOSE_BRACKET\", Delete",
                            "Insert \"PLUS\", Delete, Insert \"CLOSE_BRACKET\""]);

        let (grm, pr) = do_parse(&lexs, &grms, "n)+n+n+n)");
        let errs = pr.unwrap_err();
        assert_eq!(errs.len(), 2);
        check_repairs(&grm,
                      errs[0].repairs(),
                      &vec!["Delete, Delete, Delete, Delete"]);
        check_repairs(&grm,
                      errs[1].repairs(),
                      &vec!["Delete, Delete, Delete, Delete",
                            "Delete"]);

        let (grm, pr) = do_parse(&lexs, &grms, "(((+n)+n+n+n)");
        let errs = pr.unwrap_err();
        assert_eq!(errs.len(), 2);
        check_repairs(&grm,
                      errs[0].repairs(),
                      &vec!["Insert \"N\", Delete, Delete, Delete",
                            "Delete, Delete, Delete, Delete"]);
        check_repairs(&grm,
                      errs[1].repairs(),
                      &vec!["Insert \"CLOSE_BRACKET\""]);
    }
}