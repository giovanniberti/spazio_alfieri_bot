use std::str::FromStr;

use anyhow::{anyhow, bail, Context};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use chrono_tz::{Europe, Tz};
use itertools::Itertools;
use pest::iterators::Pair;
use pest::Parser;
use pest_derive::Parser;
use scraper::{Element, Html, Node, Selector};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgrammingEntry {
    pub title: String,
    date_entries: Vec<DateEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DateEntry {
    date: DateTime<Tz>,
    additional_details: Option<String>,
}

#[derive(Parser)]
#[grammar = "resources/date_entry.pest"]
struct DateEntryParser;

pub fn parse_email_body(body: String) -> anyhow::Result<Vec<ProgrammingEntry>> {
    parse_html(Html::parse_document(&body))
}

fn parse_html(dom: Html) -> anyhow::Result<Vec<ProgrammingEntry>> {
    let selector = Selector::parse(r#"html body.contentpane.modal div#acyarchiveview div#newsletter_preview_area.newsletter_body div.es-wrapper-color table.es-wrapper tbody tr td table.es-content tbody tr td table.es-content-body tbody tr td table tbody tr td table tbody tr td.es-m-txt-c h1"#)
        .map_err(|_| anyhow!("Invalid selector for title"))?;

    let mut entries = Vec::new();

    for title_node in dom.select(&selector) {
        let title = title_node
            .text()
            .next()
            .ok_or(anyhow!("Could not find text in selected element"))?;
        let enclosing_box = title_node
            .parent_element()
            .and_then(|e| e.parent_element())
            .and_then(|e| e.parent_element())
            .filter(|e| e.value().name() == "tbody")
            .ok_or(anyhow!(
                "Invalid element: could not find grandparent tbody box"
            ))?;

        let text = enclosing_box
            .descendants()
            .filter_map(|d| match d.value() {
                Node::Text(t) => Some(t.to_string()),
                Node::Element(e) if &e.name.local == "br" => Some(String::from("\n")),
                _ => None,
            })
            .join(" ");

        let parsed_pairs =
            DateEntryParser::parse(Rule::text, &text).context("Unable to parse text")?;
        let mut date_entries = Vec::new();
        for pair in parsed_pairs {
            match pair.as_rule() {
                Rule::date_entry => {
                    let date_entry =
                        parse_date_entry(pair).context("Unable to parse date entry")?;
                    date_entries.push(date_entry);
                }
                r => bail!("Unexpected top-level rule: {:?}", r),
            }
        }

        entries.push(ProgrammingEntry {
            title: title.to_string(),
            date_entries,
        });
    }

    Ok(entries)
}

fn parse_date_entry(pair: Pair<Rule>) -> anyhow::Result<DateEntry> {
    let mut day_number = None;
    let mut month = None;
    let mut hours = None;
    let mut minutes = None;
    let mut additional_details = None;

    for inner_pair in pair.into_inner() {
        let src = inner_pair.as_str();
        match inner_pair.as_rule() {
            Rule::day_number => {
                day_number =
                    Some(u32::from_str(src).with_context(|| {
                        format!("Unable to parse day number from value '{}'", src)
                    })?)
            }
            Rule::month => {
                let month_number = match src {
                    "gennaio" => 1,
                    "febbraio" => 2,
                    "marzo" => 3,
                    "aprile" => 4,
                    "maggio" => 5,
                    "giugno" => 6,
                    "luglio" => 7,
                    "agosto" => 8,
                    "settembre" => 9,
                    "ottobre" => 10,
                    "novembre" => 11,
                    "dicembre" => 12,
                    _ => bail!("Encountered invalid month: '{}'", src),
                };

                month = Some(month_number);
            }
            Rule::additional_details => {
                additional_details = Some(src.to_string());
            }
            Rule::time => {
                (hours, minutes) = parse_time(inner_pair)
                    .with_context(|| format!("Unable to parse time from '{}'", src))?;
            }
            Rule::hours => {
                hours = Some(
                    u32::from_str(src)
                        .with_context(|| format!("Unable to parse hours from value '{}'", src))?,
                )
            }
            Rule::minutes => {
                minutes = Some(
                    u32::from_str(src)
                        .with_context(|| format!("Unable to parse minutes from value '{}'", src))?,
                )
            }
            r => bail!("Unexpected rule: '{:?}'", r),
        }
    }

    match (day_number, month, hours, minutes, additional_details) {
        (Some(day), _, Some(hours), Some(minutes), additional_details) => {
            let now = Utc::now().with_timezone(&Europe::Rome);
            let month = month.unwrap_or(now.month());
            let date = Europe::Rome
                .with_ymd_and_hms(now.year(), month, day, hours, minutes, 0)
                .single()
                .with_context(|| {
                    format!(
                        "Unable to get valid date for y-m-d h:m:s = {}-{}-{} {}:{}:{}",
                        now.year(),
                        month,
                        day,
                        hours,
                        minutes,
                        0
                    )
                })?;

            Ok(DateEntry {
                date,
                additional_details,
            })
        }
        _ => {
            let missing_day_str = match day_number {
                Some(_) => None,
                None => Some("day"),
            };
            let missing_hours_str = match hours {
                Some(_) => None,
                None => Some("hours"),
            };
            let missing_minutes_str = match minutes {
                Some(_) => None,
                None => Some("minutes"),
            };

            let missing_fields = [missing_day_str, missing_hours_str, missing_minutes_str]
                .into_iter()
                .filter_map(|x| x)
                .collect::<Vec<_>>();
            bail!("Missing required data: {:?}", missing_fields);
        }
    }
}

fn parse_time(time: Pair<Rule>) -> anyhow::Result<(Option<u32>, Option<u32>)> {
    let mut hours = None;
    let mut minutes = None;

    for pair in time.into_inner() {
        let src = pair.as_str();
        match pair.as_rule() {
            Rule::hours => {
                hours = Some(
                    u32::from_str(src)
                        .with_context(|| format!("Unable to parse hours from value '{}'", src))?,
                )
            }
            Rule::minutes => {
                minutes = Some(
                    u32::from_str(src)
                        .with_context(|| format!("Unable to parse minutes from value '{}'", src))?,
                )
            }
            r => bail!("Encountered unexpected parsing rule: {:?}", r),
        }
    }

    Ok((hours, minutes))
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use chrono_tz::Europe;
    use std::fs::File;
    use std::io::Read;

    use tracing_test::traced_test;

    use crate::parser;
    use crate::parser::{DateEntry, ProgrammingEntry};

    #[traced_test]
    #[test]
    fn parser_with_snapshot_returns_expected_result() {
        let mut f = File::open("./tests/resources/test1.html").unwrap();
        let mut file_contents = String::new();
        f.read_to_string(&mut file_contents).unwrap();

        let entries = parser::parse_email_body(file_contents).unwrap();

        let expected_output: Vec<ProgrammingEntry> = vec![
            ProgrammingEntry {
                title: "LA SINDROME DEGLI AMORI PASSATI".to_string(),
                date_entries: vec![DateEntry {
                    date: DateTime::parse_from_rfc3339("2024-09-25T17:00:00+02:00")
                        .unwrap()
                        .with_timezone(&Europe::Rome),
                    additional_details: None,
                }],
            },
            ProgrammingEntry {
                title: "MARIA MONTESSORI".to_string(),
                date_entries: vec![
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-25T21:00:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-26T17:00:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-27T21:15:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-28T19:15:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-29T19:15:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-30T17:15:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-10-01T17:30:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-10-02T21:15:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                ],
            },
            ProgrammingEntry {
                title: "LA BAMBINA SEGRETA".to_string(),
                date_entries: vec![
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-25T18:45:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-27T15:30:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-28T17:30:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                ],
            },
            ProgrammingEntry {
                title: "MAKING OF".to_string(),
                date_entries: vec![
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-26T15:00:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-27T19:00:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: Some(
                            "—  versione originale con sottotitoli".to_string(),
                        ),
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-28T21:15:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-29T17:00:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-10-01T21:15:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: Some(
                            "—  versione originale con sottotitoli".to_string(),
                        ),
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-10-02T19:00:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                ],
            },
            ProgrammingEntry {
                title: "GLORIA MUNDI".to_string(),
                date_entries: vec![DateEntry {
                    date: DateTime::parse_from_rfc3339("2024-09-26T19:00:00+02:00")
                        .unwrap()
                        .with_timezone(&Europe::Rome),
                    additional_details: None,
                }],
            },
            ProgrammingEntry {
                title: "CUORI LIBERI".to_string(),
                date_entries: vec![
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-26T21:15:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-29T21:15:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: None,
                    },
                ],
            },
            ProgrammingEntry {
                title: "LA MOGLIE DELL'AVIATORE".to_string(),
                date_entries: vec![
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-09-30T19:15:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: Some(
                            "— versione originale con sottotitoli".to_string(),
                        ),
                    },
                    DateEntry {
                        date: DateTime::parse_from_rfc3339("2024-10-02T17:00:00+02:00")
                            .unwrap()
                            .with_timezone(&Europe::Rome),
                        additional_details: Some(
                            "— versione originale con sottotitoli".to_string(),
                        ),
                    },
                ],
            },
            ProgrammingEntry {
                title: "MARIUS E JEANNETTE".to_string(),
                date_entries: vec![DateEntry {
                    date: DateTime::parse_from_rfc3339("2024-09-30T21:15:00+02:00")
                        .unwrap()
                        .with_timezone(&Europe::Rome),
                    additional_details: None,
                }],
            },
        ];

        assert_eq!(entries, expected_output);
    }
}
