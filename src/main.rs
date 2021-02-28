mod urlgen;
use bytes::BufMut;
use futures::TryStreamExt;
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
use serde::{Deserialize, Serialize};
use sqlx::mysql::{MySqlPool, MySqlPoolOptions};
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
        .and(warp::any().map(move || pool.clone()))
        .and_then(upload);

    // upload page
    let upload_page = warp::path::end()
        .and(warp::get())
        .and(warp::fs::file("./static/upload.html"));

    // downloads
    let download_route = warp::get().and(warp::fs::dir("./files/"));

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

    println!("Binding on {}:{}", bind_ip, port);

    // https is false if either ssl key or cert are missing
    if https {
        println!("Using HTTPS");
        warp::serve(router)
            .tls()
            .cert_path(ssl_cert_path)
            .key_path(ssl_key_path)
            .run((bind_ip, port))
            .await;
    } else {
        println!("Using HTTP");
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

// handler for uploading files
async fn upload(
    form: FormData,
    settings: config::Config,
    pool: MySqlPool,
) -> Result<impl Reply, Rejection> {
    let parts: Vec<Part> = form.try_collect().await.map_err(|e| {
        eprintln!("form error: {}", e);
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

    let base_url = settings.get_str("domain").unwrap();

    for p in parts {
        println!("{}", p.name());
        if p.name() == "password" {
            let value = p
                .stream()
                .try_fold(Vec::new(), |mut vec, data| {
                    vec.put(data);
                    async move { Ok(vec) }
                })
                .await
                .map_err(|e| {
                    eprintln!("reading password error: {}", e);
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
                    eprintln!("reading file error: {}", e);
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
                        eprintln!("{}", err);
                        result.content = err;
                        break;
                    }
                },
                None => {
                    err = "File type could not be determined".to_string();
                    eprintln!("{}", err);
                    result.content = err;
                    break;
                }
            }

            file_name = format!("/{}.{}", urlgen::generate(), file_ending);
            file_content = value;
        }
    }
    if password == String::new() {
        err = "Please supply an authentication token".to_string();
        eprintln!("{}", err);
        result.content = err;
    } else {
        let exists = match sqlx::query("SELECT * FROM authentication WHERE api_key = ?")
            .bind(&password)
            .fetch_one(&pool)
            .await
        {
            Ok(_) => true,
            Err(_) => false,
        };
        println!("{} {}", password, exists);
        if !exists {
            err = "Invalid authentication token".to_string();
            eprintln!("{}", err);
            result.content = err;
        } else if file_content.len() > 0 && file_name != String::new() {
            tokio::fs::write("./files".to_string() + &file_name, file_content)
                .await
                .map_err(|e| {
                    eprintln!("Error writing file: {}", e);
                    warp::reject::reject()
                })?;
            let file_url = format!("{}{}", base_url, file_name);
            println!("Created file: {}", file_url);
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
        eprintln!("Unhandled error: {:?}", err);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error".to_string(),
        )
    };

    let result = UploadResponse {
        success: false,
        content: format!("{} {}", code, message),
    };
    Ok(warp::reply::json(&result))
}
