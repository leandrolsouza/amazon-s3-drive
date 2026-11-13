use crate::language::tr;
use crate::models::AwsCredentials;
use aws_credential_types::Credentials;
use aws_sdk_costexplorer::types::{
    DateInterval, Dimension, DimensionValues, Expression, Granularity, GroupDefinition,
    GroupDefinitionType,
};
use chrono::{Datelike, Utc};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

#[derive(Serialize)]
pub struct PriceTier {
    start_gb: f64,
    end_gb: Option<f64>,
    usd_per_gb: f64,
}

#[derive(Serialize)]
pub struct PublicPrices {
    storage: Vec<PriceTier>,
    transfer: Vec<PriceTier>,
}

fn price_tiers(offer: &Value, matches: impl Fn(&Value) -> bool, unit: &str) -> Vec<PriceTier> {
    let Some(products) = offer["products"].as_object() else {
        return Vec::new();
    };
    let mut tiers = Vec::new();
    for (sku, product) in products {
        if !matches(&product["attributes"]) {
            continue;
        }
        let Some(terms) = offer["terms"]["OnDemand"][sku].as_object() else {
            continue;
        };
        for term in terms.values() {
            let Some(dimensions) = term["priceDimensions"].as_object() else {
                continue;
            };
            for dimension in dimensions.values() {
                if dimension["unit"] != unit {
                    continue;
                }
                let Some(start_gb) = dimension["beginRange"]
                    .as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                else {
                    continue;
                };
                let Some(usd_per_gb) = dimension["pricePerUnit"]["USD"]
                    .as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                else {
                    continue;
                };
                let end_gb = match dimension["endRange"].as_str() {
                    Some("Inf") => None,
                    Some(end) => match end.parse::<f64>() {
                        Ok(value) => Some(value),
                        Err(_) => continue,
                    },
                    None => continue,
                };
                tiers.push(PriceTier {
                    start_gb,
                    end_gb,
                    usd_per_gb,
                });
            }
        }
    }
    tiers.sort_by(|a, b| a.start_gb.total_cmp(&b.start_gb));
    tiers
}

pub async fn public_prices(region: &str, storage_class: &str) -> Result<PublicPrices, String> {
    if region.is_empty()
        || !region
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err(tr("Invalid AWS region.", "Região AWS inválida.").into());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| tr("Could not start the rate lookup.", "Não foi possível iniciar a consulta de tarifas."))?;
    let (s3, data_transfer) = tokio::try_join!(
        load_offer(&client, "AmazonS3", region),
        load_offer(&client, "AWSDataTransfer", region),
    )?;
    let volume_type = match storage_class {
        "STANDARD" | "" => "Standard",
        "STANDARD_IA" => "Standard - Infrequent Access",
        "GLACIER_IR" => "Glacier Instant Retrieval",
        "INTELLIGENT_TIERING" => "Intelligent-Tiering Frequent Access",
        _ => "",
    };
    let storage = price_tiers(
        &s3,
        |a| !volume_type.is_empty() && a["regionCode"] == region && a["volumeType"] == volume_type,
        "GB-Mo",
    );
    let transfer = price_tiers(
        &data_transfer,
        |a| {
            a["fromRegionCode"] == region
                && a["transferType"] == "AWS Outbound"
                && a["toLocation"] == "External"
        },
        "GB",
    );
    Ok(PublicPrices { storage, transfer })
}

async fn load_offer(
    client: &reqwest::Client,
    service: &str,
    region: &str,
) -> Result<Value, String> {
    let url = format!("https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/{service}/current/{region}/index.json");
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| tr("The AWS price list is unavailable.", "Catálogo de tarifas da AWS indisponível."))?;
    response
        .json()
        .await
        .map_err(|_| tr("The AWS price list is invalid.", "Catálogo de tarifas da AWS inválido.").into())
}

#[derive(Serialize)]
pub struct UsageCost {
    usage_type: String,
    usd: f64,
}

#[derive(Serialize)]
pub struct MonthlySpend {
    fetched_at: String,
    start: String,
    end_exclusive: String,
    estimated: bool,
    partial: bool,
    rows: Vec<UsageCost>,
}

pub async fn monthly_spend(creds: Option<AwsCredentials>) -> Result<MonthlySpend, String> {
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new("us-east-1"))
        .retry_config(aws_config::retry::RetryConfig::standard().with_max_attempts(1));
    if let Some(creds) = creds {
        loader = loader.credentials_provider(Credentials::new(
            creds.access_key_id,
            creds.secret_access_key,
            creds.session_token,
            None,
            "amazon-s3-drive",
        ));
    }
    let config = loader.load().await;
    let client = aws_sdk_costexplorer::Client::new(&config);
    let now = Utc::now();
    let start = now.date_naive().with_day(1).unwrap().to_string();
    let end = now.date_naive().succ_opt().unwrap().to_string();
    let result = client.get_cost_and_usage()
        .time_period(DateInterval::builder().start(&start).end(&end).build().map_err(|_| tr("Invalid date range.", "Período inválido."))?)
        .granularity(Granularity::Monthly)
        .metrics("UnblendedCost")
        .filter(Expression::builder().dimensions(DimensionValues::builder()
            .key(Dimension::Service).values("Amazon Simple Storage Service").build()).build())
        .group_by(GroupDefinition::builder().r#type(GroupDefinitionType::Dimension).key("USAGE_TYPE").build())
        .send().await.map_err(|_| tr("Could not query Cost Explorer. Check that it is enabled and that your credentials have the ce:GetCostAndUsage permission.", "Não foi possível consultar o Cost Explorer. Confira se ele está ativo e se suas credenciais têm a permissão ce:GetCostAndUsage."))?;
    let mut rows = Vec::new();
    for period in result.results_by_time() {
        for group in period.groups() {
            let Some(usage_type) = group.keys().first() else {
                continue;
            };
            let usd = group
                .metrics()
                .and_then(|m| m.get("UnblendedCost"))
                .filter(|value| value.unit() == Some("USD"))
                .and_then(|value| value.amount())
                .and_then(|amount| amount.parse::<f64>().ok())
                .ok_or(
                    tr("Cost Explorer returned an amount with a missing USD unit or an invalid format.", "O Cost Explorer retornou um valor sem unidade USD ou em formato inválido."),
                )?;
            rows.push(UsageCost {
                usage_type: usage_type.clone(),
                usd,
            });
        }
    }
    rows.sort_by(|a, b| b.usd.abs().total_cmp(&a.usd.abs()));
    Ok(MonthlySpend {
        fetched_at: Utc::now().to_rfc3339(),
        start,
        end_exclusive: end,
        estimated: result
            .results_by_time()
            .iter()
            .any(|p| p.estimated()),
        partial: result
            .next_page_token()
            .is_some_and(|token| !token.is_empty()),
        rows,
    })
}
