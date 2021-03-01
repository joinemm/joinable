mod urlgen;
use bytes::BufMut;
use futures::TryStreamExt;
use log::LevelFilter;
use mime_sniffer::MimeTypeSniffer;
use std::convert::Infallible;
use std::fs;
use std::net::Ipv4Addr;
use warp::{
    http::StatusCode,
    multipart::{FormData, Part},
    Filter, Rejection, Reply,
};
extern crate config;
extern crate pretty_env_logger;
#[macro_use]
extern crate log;
use serde::{Deserialize, Serialize};
use sqlx::mysql::{MySqlPool, MySqlPoolOptions};
use sqlx::Row;
use std::error::Error;

pub async fn get_pool(database_url: &str) -> Result<MySqlPool, Box<dyn Error>> {
    let pool = MySqlPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;
    Ok(pool)
}

#[derive(Serialize, Deserialize, Debug)]
struct UploadResponse {
    success: bool,
    content: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    pretty_env_logger::formatted_timed_builder()
        .filter(None, LevelFilter::Info)
        .init();
    let mut settings = config::Config::default();
    settings
        // Add in `./Settings.toml`
        .merge(config::File::with_name("Settings"))
        .unwrap()
        // Add in settings from the environment (with a prefix of APP)
        // Eg.. `APP_DEBUG=1 ./target/app` would set the `debug` key
        .merge(config::Environment::with_prefix("APP"))
        .unwrap();

    // create files folder of it doesnt exist
    fs::create_dir_all("./files").unwrap();

    let pool = get_pool(&settings.get_str("db_connection").unwrap())
        .await
        .unwrap();

    // uploads
    let upload_route = warp::path("upload")
        .and(warp::post())
        .and(warp::multipart::form().max_length(settings.get_int("max_file_size").unwrap() as u64))
        .and(with_settings(settings.clone()))
        .and(with_db(pool.clone()))
        .and_then(upload);

    // upload page
    let upload_page = warp::path::end()
        .and(warp::get())
        .and(warp::fs::file("./static/upload.html"));

    // downloads
    let download_route = warp::get()
        .and(warp::fs::dir("./files/"))
        .and(with_db(pool.clone()))
        .and_then(log_access);

    // static cdn
    let static_files = warp::path("static")
        .and(warp::get())
        .and(warp::fs::dir("./static"));

    let router = upload_route
        .or(download_route)
        .or(upload_page)
        .or(static_files)
        .recover(handle_rejection);

    let port = match settings.get_int("port") {
        Ok(v) => v as u16,
        Err(_) => 8080,
    };

    let mut https = match settings.get_bool("use_https") {
        Ok(v) => v,
        Err(_) => true,
    };
    let ssl_cert_path = match settings.get_str("ssl_cert") {
        Ok(v) => v,
        Err(_) => {
            https = false;
            "".to_string()
        }
    };
    let ssl_key_path = match settings.get_str("ssl_key") {
        Ok(v) => v,
        Err(_) => {
            https = false;
            "".to_string()
        }
    };

    let bind_ip = match settings.get_str("ip") {
        Ok(v) => v.parse::<Ipv4Addr>().unwrap(),
        Err(_) => Ipv4Addr::LOCALHOST,
    };

    // https is false if either ssl key or cert are missing
    if https {
        warp::serve(router)
            .tls()
            .cert_path(ssl_cert_path)
            .key_path(ssl_key_path)
            .run((bind_ip, port))
            .await;
    } else {
        warp::serve(router).run((bind_ip, port)).await;
    }
    Ok(())
}

// function to pass settings into a warp handler
fn with_settings(
    settings: config::Config,
) -> impl Filter<Extract = (config::Config,), Error = std::convert::Infallible> + Clone {
    warp::any().map(move || settings.clone())
}

fn with_db(db_pool: MySqlPool) -> impl Filter<Extract = (MySqlPool,), Error = Infallible> + Clone {
    warp::any().map(move || db_pool.clone())
}

async fn log_access(file: warp::fs::File, pool: MySqlPool) -> Result<impl Reply, Rejection> {
    let file_id = file.path().file_stem().unwrap().to_str().unwrap();
    println!("{}", &file_id);
    let result = sqlx::query(
        "UPDATE upload
        SET last_accessed = NOW(),
        times_accessed = times_accessed + 1
        WHERE identifier = ?",
    )
    .bind(&file_id)
    .execute(&pool)
    .await;
    match result {
        Err(e) => error!("{}", e),
        _ => (),
    }
    Ok(file)
}

// handler for uploading files
async fn upload(
    form: FormData,
    settings: config::Config,
    pool: MySqlPool,
) -> Result<impl Reply, Rejection> {
    let parts: Vec<Part> = form.try_collect().await.map_err(|e| {
        error!("form error: {}", e);
        warp::reject::reject()
    })?;

    let mut result = UploadResponse {
        success: false,
        content: String::new(),
    };
    let mut err: String;
    let mut password = String::new();
    let mut file_content = Vec::<u8>::new();
    let mut file_name = String::new();
    let mut file_id = String::new();

    let base_url = settings.get_str("domain").unwrap();

    for p in parts {
        if p.name() == "password" {
            let value = p
                .stream()
                .try_fold(Vec::new(), |mut vec, data| {
                    vec.put(data);
                    async move { Ok(vec) }
                })
                .await
                .map_err(|e| {
                    error!("reading password error: {}", e);
                    warp::reject::reject()
                })?;
            password = String::from_utf8(value).unwrap();
        } else if p.name() == "file" {
            // read actual file stream into a byte vector
            let value = p
                .stream()
                .try_fold(Vec::new(), |mut vec, data| {
                    vec.put(data);
                    async move { Ok(vec) }
                })
                .await
                .map_err(|e| {
                    error!("reading file error: {}", e);
                    warp::reject::reject()
                })?;

            // determine content type and file extension
            let content_type = value.sniff_mime_type();
            let file_ending;
            match content_type {
                Some(file_type) => match file_type {
                    // supported file types
                    "image/png" => {
                        file_ending = "png";
                    }
                    "image/jpeg" => {
                        file_ending = "jpeg";
                    }
                    "image/gif" => {
                        file_ending = "gif";
                    }
                    "image/webp" => {
                        file_ending = "webp";
                    }
                    "video/webm" => {
                        file_ending = "webm";
                    }
                    "audio/mpeg" => {
                        file_ending = "mp3";
                    }
                    "video/mp4" => {
                        file_ending = "mp4";
                    }

                    v => {
                        err = format!("Unsupported file type: {}", v);
                        warn!("{}", err);
                        result.content = err;
                        break;
                    }
                },
                None => {
                    err = "File type could not be determined".to_string();
                    warn!("{}", err);
                    result.content = err;
                    break;
                }
            }
            file_id = urlgen::generate();
            file_name = format!("/{}.{}", file_id, file_ending);
            file_content = value;
        }
    }
    if password == String::new() {
        err = "Please supply an authentication token".to_string();
        warn!("{}", err);
        result.content = err;
    } else {
        let exists = match sqlx::query("SELECT active FROM authentication WHERE api_key = ?")
            .bind(&password)
            .fetch_one(&pool)
            .await
        {
            Ok(v) => v.try_get("active").unwrap(),
            Err(_) => false,
        };
        if !exists {
            err = "Invalid authentication token".to_string();
            warn!("{}", err);
            result.content = err;
        } else if file_content.len() > 0 && file_name != String::new() {
            tokio::fs::write("./files".to_string() + &file_name, file_content)
                .await
                .map_err(|e| {
                    error!("Error writing file: {}", e);
                    warp::reject::reject()
                })?;
            let file_url = format!("{}{}", base_url, file_name);

            match sqlx::query(
                "INSERT INTO upload (identifier, created_on, api_key_used, last_accessed) VALUES (?, NOW(), ?, NOW())",
            )
            .bind(&file_id)
            .bind(&password)
            .execute(&pool)
            .await {
                Err(e) => error!("{}", e),
                _ => ()
            }

            info!("Created file: {} using api_key: {}", file_url, &password);
            result.success = true;
            result.content = file_url;
        }
    }
    Ok(warp::reply::json(&result))
}

async fn handle_rejection(err: Rejection) -> std::result::Result<impl Reply, Infallible> {
    // if statement returns values to code and message variables
    let (code, message) = if err.is_not_found() {
        (StatusCode::NOT_FOUND, "Not Found".to_string())
    } else if err.find::<warp::reject::PayloadTooLarge>().is_some() {
        (StatusCode::BAD_REQUEST, "Payload too large".to_string())
    } else {
        error!("Unhandled error: {:?}", err);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error".to_string(),
        )
    };

    if code == StatusCode::NOT_FOUND {
        Ok(warp::reply::html(
            r#"
            <html>
                <head>
                    <title>404 NOT FOUND</title>
                </head>
                <body>
                    <h1>404 NOT FOUND</h1>
                </body>
            </html>
            "#,
        )
        .into_response())
    } else {
        let result = UploadResponse {
            success: false,
            content: format!("{} {}", code, message),
        };
        Ok(warp::reply::json(&result).into_response())
    }
}
