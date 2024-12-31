use std::str::FromStr;

use anyhow::{anyhow, bail, Context};
use chrono::{DateTime, Datelike, NaiveTime, TimeZone, Utc};
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

#[derive(Debug, Clone)]
pub enum ParsedDateEntries<'r> {
    Parsed(Vec<DateEntry>),
    Uncertain(Pair<'r, Rule>),
}

#[derive(Parser)]
#[grammar = "resources/date_entry.pest"]
struct DateEntryParser;

mod subject_line {
    use pest_derive::Parser;

    #[derive(Parser)]
    #[grammar = "resources/subject_line.pest"]
    pub struct SubjectLineParser;
}

pub fn parse_email_body(subject: String, body: String) -> anyhow::Result<NewsletterEntry> {
    let date_boundaries = parse_subject_line_dates(&subject).context("Unable to parse subject line")?;
    parse_html(Html::parse_document(&body), date_boundaries)
}

fn parse_subject_line_dates(subject_line: &str) -> anyhow::Result<Vec<DateTime<Tz>>> {
    use subject_line::*;
    let parsed_pairs =
        SubjectLineParser::parse(Rule::text, subject_line)
            .context("Unable to parse text rule for subject line")?;
    let mut day_numbers = Vec::with_capacity(2);
    let mut months = Vec::new();

    for pair in parsed_pairs {
        match pair.as_rule() {
            Rule::day_number => {
                let day_number = u32::from_str(pair.as_str()).context("Unable to parse invalid `day_number` value")?;
                day_numbers.push(day_number);
            }
            Rule::month => {
                let month_number = month_name_to_number(pair.as_str())?;

                if months.is_empty() && day_numbers.is_empty() || day_numbers.len() > 2 {
                    bail!("Unexpected `month` input with invalid day numbers: {day_numbers:?}");
                }

                months.push(month_number)
            }
            r => {
                println!("Got unexpected rule: {:?}", r)
            }
        }
    }

    if day_numbers.len() != 2 {
        bail!("Expected two day numbers, got {}", day_numbers.len());
    }

    let mut dates = Vec::with_capacity(2);
    let now = Utc::now();

    if months.len() == 1 {
        months.push(months[0]);
    }

    for (day, month) in day_numbers.into_iter().zip(months) {
        let date = Europe::Rome
            .with_ymd_and_hms(now.year(), month, day, 0, 0, 0)
            .single()
            .with_context(|| {
                format!(
                    "Unable to get valid date for y-m-d = {}-{month}-{day} ",
                    now.year()
                )
            })?;

        dates.push(date);
    }

    if dates[1] < dates[0] { // handle year crossover e.g. dec 27 -> jan 3
        dates[1] = dates[1].with_year(now.year() + 1).unwrap();
    }

    dates[1] = dates[1].with_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap()).unwrap();

    Ok(dates)
}

fn parse_html(dom: Html, date_boundaries: Vec<DateTime<Tz>>) -> anyhow::Result<NewsletterEntry> {
    let [lower_bound, upper_bound] = date_boundaries[..] else {
        bail!("Invalid date boundaries: {date_boundaries:?}")
    };
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
        let mut pairs_to_reparse = Vec::new();
        let mut date_entries = Vec::new();
        for pair in parsed_pairs {
            match pair.as_rule() {
                Rule::date_entry => {
                    let parsed_date_entries =
                        parse_date_entry(pair, lower_bound, upper_bound).context("Unable to parse date entry")?;

                    match parsed_date_entries {
                        ParsedDateEntries::Parsed(parsed) => date_entries.extend(parsed),
                        ParsedDateEntries::Uncertain(to_reparse) => {
                            pairs_to_reparse.push(to_reparse)
                        }
                    }
                }
                r => bail!("Unexpected top-level rule: {:?}", r),
            }
        }

        for pair in pairs_to_reparse {
            match pair.as_rule() {
                Rule::date_entry => {
                    let parsed_date_entries = parse_date_entry(pair, lower_bound, upper_bound)
                        .context("Unable to parse date entry")?;

                    match parsed_date_entries {
                        ParsedDateEntries::Parsed(parsed) => date_entries.extend(parsed),
                        ParsedDateEntries::Uncertain(to_reparse) => bail!(
                            "Unable to parse month from input: '{}'",
                            to_reparse.as_str()
                        ),
                    }
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

fn month_name_to_number(name: &str) -> anyhow::Result<u32> {
    match name {
        "gennaio" => Ok(1),
        "febbraio" => Ok(2),
        "marzo" => Ok(3),
        "aprile" => Ok(4),
        "maggio" => Ok(5),
        "giugno" => Ok(6),
        "luglio" => Ok(7),
        "agosto" => Ok(8),
        "settembre" => Ok(9),
        "ottobre" => Ok(10),
        "novembre" => Ok(11),
        "dicembre" => Ok(12),
        _ => bail!("Encountered invalid month: '{}'", name),
    }
}

fn parse_date_entry(
    pair: Pair<Rule>,
    lower_bound: DateTime<Tz>,
    upper_bound: DateTime<Tz>,
) -> anyhow::Result<ParsedDateEntries> {
    let mut day_number = None;
    let mut month = None;
    let mut times = Vec::new();
    let mut additional_details = None;

    for inner_pair in pair.clone().into_inner() {
        let src = inner_pair.as_str();
        match inner_pair.as_rule() {
            Rule::day_number => {
                day_number =
                    Some(u32::from_str(src).with_context(|| {
                        format!("Unable to parse day number from value '{}'", src)
                    })?)
            }
            Rule::month => {
                month = Some(month_name_to_number(src)?);
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
            let date_entries = times
                .iter()
                .map(|(hours, minutes)| {
                        let first_candidate_date: Option<DateTime<Tz>> = lower_bound.with_day(day);
                        let second_candidate_date = upper_bound.with_day(day);
                        first_candidate_date
                            .filter(|d| d >= &lower_bound)
                            .or(second_candidate_date)
                            .filter(|d| d <= &upper_bound)
                            .and_then(|d| {
                                let time = NaiveTime::from_hms_opt(*hours, *minutes, 0);
                                time.and_then(|t| d.with_time(t).single())
                            })
                            .with_context(|| {
                                format!(
                                    "Unable to get valid date with day {} from bounds: [{}, {}] and time {}:{}",
                                    day,
                                    lower_bound,
                                    upper_bound,
                                    hours,
                                    minutes
                                )
                            })
                })
                .collect::<Vec<anyhow::Result::<_, _>>>()
                .into_iter()
                .filter_map(Result::ok)
                .map(|date| DateEntry {
                    date,
                    additional_details: additional_details.clone(),
                })
                .collect();

            Ok(ParsedDateEntries::Parsed(date_entries))
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

        let subject_line = "Spazio Alfieri • programmazione 25 settembre > 2 ottobre".to_string();
        let newsletter_entry = parser::parse_email_body(subject_line, file_contents).unwrap();

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
                        date: DateTime::parse_from_rfc3339("2024-09-27T17:00:00+02:00")
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
                        date: DateTime::parse_from_rfc3339("2024-09-28T15:30:00+02:00")
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
                        date: DateTime::parse_from_rfc3339("2024-09-29T15:00:00+02:00")
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
