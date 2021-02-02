use bytes::BufMut;
use futures::TryStreamExt;
use mime_sniffer::MimeTypeSniffer;
use std::convert::Infallible;
use std::fs;
use std::net::Ipv4Addr;
use uuid::Uuid;
use warp::{
    http::StatusCode,
    multipart::{FormData, Part},
    Filter, Rejection, Reply,
};

// constants
static DOMAIN: &str = "joinable.xyz";
static IP: Ipv4Addr = Ipv4Addr::new(0, 0, 0, 0);
static MAX_LENGTH: u64 = 8_000_000;

static HTTPS: bool = false;
static CERT_PATH: &str = "";
static KEY_PATH: &str = "";

#[tokio::main]
async fn main() {
    // create files folder of it doesnt exist
    fs::create_dir_all("./files").unwrap();
    // Matches requests that start with `/files`,
    // and then uses the rest of that path to lookup
    // and serve a file from `./files/`.
    let download_route = warp::path("files").and(warp::fs::dir("./files/"));

    // uploads
    let upload_route = warp::path("upload")
        .and(warp::post())
        .and(warp::multipart::form().max_length(MAX_LENGTH))
        .and_then(upload);

    let router = upload_route.or(download_route).recover(handle_rejection);

    let port;
    if HTTPS {
        port = 433;
        println!("Server started using HTTPS on port {}", port);
        warp::serve(router)
            .tls()
            .cert_path(CERT_PATH)
            .key_path(KEY_PATH)
            .run((IP, port))
            .await;
    } else {
        port = 80;
        println!("Server started using HTTP on port {}", port);
        warp::serve(router).run((IP, port)).await;
    }
}

async fn upload(form: FormData) -> Result<impl Reply, Rejection> {
    let parts: Vec<Part> = form.try_collect().await.map_err(|e| {
        eprintln!("form error: {}", e);
        warp::reject::reject()
    })?;

    let mut file_name = "".to_string();

    for p in parts {
        if p.name() == "file" {
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

                    "video/mp4" => {
                        file_ending = "mp4";
                    }

                    v => {
                        eprintln!("invalid file type: {}", v);
                        return Err(warp::reject::reject());
                    }
                },
                None => {
                    eprintln!("file type could not be determined");
                    return Err(warp::reject::reject());
                }
            }

            file_name = format!("/files/{}.{}", Uuid::new_v4().to_string(), file_ending);
            tokio::fs::write(".".to_string() + &file_name, value)
                .await
                .map_err(|e| {
                    eprintln!("error writing file: {}", e);
                    warp::reject::reject()
                })?;
            println!("created file: {}", file_name);
        }
    }

    Ok(format!(
        "Success! Your file is at https://{}{}\n",
        DOMAIN, file_name
    ))
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

    Ok(warp::reply::with_status(message, code))
}
