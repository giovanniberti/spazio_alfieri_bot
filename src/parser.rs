use std::str::FromStr;

use anyhow::{anyhow, bail, Context};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use chrono_tz::{Europe, Tz};
use itertools::Itertools;
use pest::iterators::Pair;
use pest::Parser;
use pest_derive::Parser;
use scraper::{Element, Html, Node, Selector};
use tracing::info;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsletterEntry {
    pub programming_entries: Vec<ProgrammingEntry>,
    pub newsletter_link: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgrammingEntry {
    pub title: String,
    pub date_entries: Vec<DateEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateEntry {
    pub date: DateTime<Tz>,
    pub additional_details: Option<String>,
}

#[derive(Parser)]
#[grammar = "resources/date_entry.pest"]
struct DateEntryParser;

pub fn parse_email_body(body: String) -> anyhow::Result<NewsletterEntry> {
    parse_html(Html::parse_document(&body))
}

fn parse_html(dom: Html) -> anyhow::Result<NewsletterEntry> {
    let newsletter_link_selector =
        Selector::parse(r#"table > tbody > tr > td > table > tbody > tr > td > p > a"#).unwrap();
    let newsletter_link = dom
        .select(&newsletter_link_selector)
        .next()
        .ok_or(anyhow!("Could not find newsletter link!"))?
        .attr("href")
        .ok_or(anyhow!("Newsletter link doesn't have `href` attribute!"))?;

    let title_selector = Selector::parse(r#"div div div table tbody tr td table tbody tr td table tbody tr td table tbody tr td table tbody tr td h1"#)
        .map_err(|_| anyhow!("Invalid selector for title"))?;

    let mut entries = Vec::new();

    let title_nodes = dom.select(&title_selector).collect::<Vec<_>>();
    info!("Got {} title nodes", title_nodes.len());

    for title_node in title_nodes {
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
                    let parsed_date_entries =
                        parse_date_entry(pair).context("Unable to parse date entry")?;
                    date_entries.extend(parsed_date_entries);
                }
                r => bail!("Unexpected top-level rule: {:?}", r),
            }
        }

        entries.push(ProgrammingEntry {
            title: title.to_string(),
            date_entries,
        });
    }

    Ok(NewsletterEntry {
        programming_entries: entries,
        newsletter_link: newsletter_link.into(),
    })
}

fn parse_date_entry(pair: Pair<Rule>) -> anyhow::Result<Vec<DateEntry>> {
    let mut day_number = None;
    let mut month = None;
    let mut times = Vec::new();
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
                let (hours, minutes) = parse_time(inner_pair)
                    .with_context(|| format!("Unable to parse time from '{}'", src))?;

                times.push((hours, minutes));
            }
            r => bail!("Unexpected rule: '{:?}'", r),
        }
    }

    match (day_number, month, &times, additional_details) {
        (Some(day), _, times, additional_details) if !times.is_empty() => {
            let now = Utc::now().with_timezone(&Europe::Rome);
            let month = month.unwrap_or(now.month());
            let date_entries = times
                .into_iter()
                .map(|(hours, minutes)| {
                    Europe::Rome
                        .with_ymd_and_hms(now.year(), month, day, *hours, *minutes, 0)
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
                        })
                })
                .collect::<anyhow::Result<Vec<_>>>()?
                .into_iter()
                .map(|date| DateEntry {
                    date,
                    additional_details: additional_details.clone(),
                })
                .collect();

            Ok(date_entries)
        }
        _ => {
            let missing_day_str = match day_number {
                Some(_) => None,
                None => Some("day"),
            };
            let missing_times_str = match times.is_empty() {
                false => None,
                true => Some("times"),
            };

            let missing_fields = [missing_day_str, missing_times_str]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            bail!("Missing required data: {:?}", missing_fields);
        }
    }
}

fn parse_time(time: Pair<Rule>) -> anyhow::Result<(u32, u32)> {
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

    Ok((
        hours.ok_or(anyhow!("Missing hours rule inside time component"))?,
        minutes.ok_or(anyhow!("Missing minutes rule inside time component"))?,
    ))
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use chrono_tz::Europe;
    use std::fs::File;
    use std::io::Read;

    use tracing_test::traced_test;

    use crate::parser;
    use crate::parser::{DateEntry, NewsletterEntry, ProgrammingEntry};

    #[traced_test]
    #[test]
    fn parser_with_snapshot_returns_expected_result() {
        let mut f = File::open("./tests/resources/test1.html").unwrap();
        let mut file_contents = String::new();
        f.read_to_string(&mut file_contents).unwrap();

        let newsletter_entry = parser::parse_email_body(file_contents).unwrap();

        let expected_link = "https://6534.sqm-secure.eu/index.php?option=com_acymailing&ctrl=archive&task=view&mailid=231&key=FdgUJqRewx&subid=5789-00898287&tmpl=component&lang=it&utm_source=newsletter_231&utm_medium=email&utm_campaign=newsletter-24-30-novembre&acm=5789_231";
        let expected_entries: Vec<ProgrammingEntry> = vec![
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

        let expected_output = NewsletterEntry {
            programming_entries: expected_entries,
            newsletter_link: expected_link.into(),
        };
        assert_eq!(newsletter_entry, expected_output);
    }
}
