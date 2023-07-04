#[macro_use]
extern crate rocket;
use dotenvy::dotenv;
use rocket::fairing::{Fairing, Info, Kind};
use rocket::figment::{
    util::map,
    value::{Map, Value},
};
use rocket::http::Header;
use rocket::http::Status;
use rocket::response::status;
use rocket::serde::json::Json;
use rocket::{Request, Response};
use rsky_feedgen::{ReadReplicaConn, WriteDbConn};
use std::env;
use rsky_feedgen::models::JwtParts;

pub struct CORS;

use rocket::request::{FromRequest, Outcome};

struct ApiKey<'r>(&'r str);

#[derive(Debug)]
struct AccessToken(String);

#[derive(Debug)]
enum ApiKeyError {
    Missing,
    Invalid,
}

#[derive(Debug)]
enum AccessTokenError {
    Missing,
    Invalid,
}

#[allow(unused_assignments)]
#[rocket::async_trait]
impl<'r> FromRequest<'r> for ApiKey<'r> {
    type Error = ApiKeyError;

    async fn from_request(req: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let mut token: String = "".to_owned();
        if let Ok(token_result) = env::var("RSKY_API_KEY") {
            token = token_result;
        } else {
            return Outcome::Failure((Status::BadRequest, ApiKeyError::Invalid));
        }

        match req.headers().get_one("X-RSKY-KEY") {
            None => Outcome::Failure((Status::Unauthorized, ApiKeyError::Missing)),
            Some(key) if key == token => Outcome::Success(ApiKey(key)),
            Some(_) => Outcome::Failure((Status::Unauthorized, ApiKeyError::Invalid)),
        }
    }
}

#[allow(unused_assignments)]
#[rocket::async_trait]
impl<'r> FromRequest<'r> for AccessToken {
    type Error = AccessTokenError;

    async fn from_request(req: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        match req.headers().get_one("Authorization") {
            None => Outcome::Failure((Status::Unauthorized, AccessTokenError::Missing)),
            Some(token) if !token.starts_with("Bearer ") => {
                Outcome::Failure((Status::Unauthorized, AccessTokenError::Invalid))
            }
            Some(token) => {
                let service_did = env::var("FEEDGEN_SERVICE_DID").unwrap_or("".into());
                let jwt = token
                    .split(" ")
                    .map(String::from)
                    .collect::<Vec<_>>();
                if let Some(jwtstr) = jwt.last() {
                    match rsky_feedgen::auth::verify_jwt(&jwtstr, &service_did) {
                        Ok(jwt_object) => Outcome::Success(AccessToken(jwt_object)),
                        Err(error) => {
                            eprintln!("Error decoding jwt.");
                            Outcome::Failure((Status::Unauthorized, AccessTokenError::Invalid))
                        },
                    }
                } else {
                    Outcome::Failure((Status::Unauthorized, AccessTokenError::Invalid))
                }
            }
        }
    }
}

const BLACKSKY: &str = "at://did:plc:w4xbfzo7kqfes5zb7r6qv3rw/app.bsky.feed.generator/blacksky";

#[get(
    "/xrpc/app.bsky.feed.getFeedSkeleton?<feed>&<limit>&<cursor>",
    format = "json"
)]
async fn index(
    feed: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
    connection: ReadReplicaConn,
    _token: Result<AccessToken, AccessTokenError>,
) -> Result<
    Json<rsky_feedgen::models::AlgoResponse>,
    status::Custom<Json<rsky_feedgen::models::InternalErrorMessageResponse>>,
> {
    if let Ok(jwt) = _token {
        match serde_json::from_str::<JwtParts>(&jwt.0) {
            Ok(jwt_obj) => {
                match rsky_feedgen::apis::add_visitor(jwt_obj.iss, jwt_obj.aud) {
                    Ok(_) => (),
                    Err(error) => eprintln!("Failed to write visitor."),
                }
            },
            Err(error) => eprintln!("Failed to parse jwt string."),
        }
    } else {
        let service_did = env::var("FEEDGEN_SERVICE_DID").unwrap_or("".into());
        match rsky_feedgen::apis::add_visitor("anonymous".into(), service_did) {
            Ok(_) => (),
            Err(error) => eprintln!("Failed to write anonymous visitor."),
        }
    }
    let _blacksky: String = String::from(BLACKSKY);
    match feed {
        Some(_blacksky) => {
            match rsky_feedgen::apis::get_blacksky_posts(limit, cursor, connection).await {
                Ok(response) => Ok(Json(response)),
                Err(error) => {
                    eprintln!("Internal Error: {error}");
                    let internal_error = rsky_feedgen::models::InternalErrorMessageResponse {
                        code: Some(rsky_feedgen::models::InternalErrorCode::InternalError),
                        message: Some(error.to_string()),
                    };
                    Err(status::Custom(
                        Status::InternalServerError,
                        Json(internal_error),
                    ))
                }
            }
        }
        _ => {
            let internal_error = rsky_feedgen::models::InternalErrorMessageResponse {
                code: Some(rsky_feedgen::models::InternalErrorCode::InternalError),
                message: Some("Not Found".to_string()),
            };
            Err(status::Custom(
                Status::InternalServerError,
                Json(internal_error),
            ))
        }
    }
}

#[put("/cursor?<service>&<sequence>")]
async fn update_cursor(
    service: String,
    sequence: i64,
    _key: ApiKey<'_>,
    connection: WriteDbConn,
) -> Result<(), status::Custom<Json<rsky_feedgen::models::InternalErrorMessageResponse>>> {
    match rsky_feedgen::apis::update_cursor(service, sequence, connection).await {
        Ok(_) => Ok(()),
        Err(error) => {
            eprintln!("Internal Error: {error}");
            let internal_error = rsky_feedgen::models::InternalErrorMessageResponse {
                code: Some(rsky_feedgen::models::InternalErrorCode::InternalError),
                message: Some(error.to_string()),
            };
            Err(status::Custom(
                Status::InternalServerError,
                Json(internal_error),
            ))
        }
    }
}

#[get("/cursor?<service>", format = "json")]
async fn get_cursor(
    service: String,
    _key: ApiKey<'_>,
    connection: ReadReplicaConn,
) -> Result<
    Json<rsky_feedgen::models::SubState>,
    status::Custom<Json<rsky_feedgen::models::PathUnknownErrorMessageResponse>>,
> {
    match rsky_feedgen::apis::get_cursor(service, connection).await {
        Ok(response) => Ok(Json(response)),
        Err(error) => {
            eprintln!("Internal Error: {error}");
            let path_error = rsky_feedgen::models::PathUnknownErrorMessageResponse {
                code: Some(rsky_feedgen::models::NotFoundErrorCode::NotFoundError),
                message: Some("Not Found".to_string()),
            };
            Err(status::Custom(Status::NotFound, Json(path_error)))
        }
    }
}

#[put("/queue/create", format = "json", data = "<body>")]
async fn queue_creation(
    body: Json<Vec<rsky_feedgen::models::CreateRequest>>,
    _key: ApiKey<'_>,
    connection: WriteDbConn,
) -> Result<(), status::Custom<Json<rsky_feedgen::models::InternalErrorMessageResponse>>> {
    match rsky_feedgen::apis::queue_creation(body.into_inner(), connection).await {
        Ok(_) => Ok(()),
        Err(error) => {
            eprintln!("Internal Error: {error}");
            let internal_error = rsky_feedgen::models::InternalErrorMessageResponse {
                code: Some(rsky_feedgen::models::InternalErrorCode::InternalError),
                message: Some(error.to_string()),
            };
            Err(status::Custom(
                Status::InternalServerError,
                Json(internal_error),
            ))
        }
    }
}

#[put("/queue/delete", format = "json", data = "<body>")]
async fn queue_deletion(
    body: Json<Vec<rsky_feedgen::models::DeleteRequest>>,
    _key: ApiKey<'_>,
    connection: WriteDbConn,
) -> Result<(), status::Custom<Json<rsky_feedgen::models::InternalErrorMessageResponse>>> {
    match rsky_feedgen::apis::queue_deletion(body.into_inner(), connection).await {
        Ok(_) => Ok(()),
        Err(error) => {
            eprintln!("Internal Error: {error}");
            let internal_error = rsky_feedgen::models::InternalErrorMessageResponse {
                code: Some(rsky_feedgen::models::InternalErrorCode::InternalError),
                message: Some(error.to_string()),
            };
            Err(status::Custom(
                Status::InternalServerError,
                Json(internal_error),
            ))
        }
    }
}

#[get("/.well-known/did.json", format = "json")]
async fn well_known() -> Result<
    Json<rsky_feedgen::models::WellKnown>,
    status::Custom<Json<rsky_feedgen::models::PathUnknownErrorMessageResponse>>,
> {
    match env::var("FEEDGEN_SERVICE_DID") {
        Ok(service_did) => {
            let hostname = env::var("FEEDGEN_HOSTNAME").unwrap_or("".into());
            if !service_did.ends_with(hostname.as_str()) {
                let path_error = rsky_feedgen::models::PathUnknownErrorMessageResponse {
                    code: Some(rsky_feedgen::models::NotFoundErrorCode::NotFoundError),
                    message: Some("Not Found".to_string()),
                };
                Err(status::Custom(Status::NotFound, Json(path_error)))
            } else {
                let known_service = rsky_feedgen::models::KnownService {
                    id: "#bsky_fg".to_owned(),
                    r#type: "BskyFeedGenerator".to_owned(),
                    service_endpoint: format!("https://{}", hostname),
                };
                let result = rsky_feedgen::models::WellKnown {
                    context: vec!["https://www.w3.org/ns/did/v1".into()],
                    id: service_did,
                    service: vec![known_service],
                };
                Ok(Json(result))
            }
        }
        Err(_) => {
            let path_error = rsky_feedgen::models::PathUnknownErrorMessageResponse {
                code: Some(rsky_feedgen::models::NotFoundErrorCode::NotFoundError),
                message: Some("Not Found".to_string()),
            };
            Err(status::Custom(Status::NotFound, Json(path_error)))
        }
    }
}

#[catch(404)]
async fn not_found() -> Json<rsky_feedgen::models::PathUnknownErrorMessageResponse> {
    let path_error = rsky_feedgen::models::PathUnknownErrorMessageResponse {
        code: Some(rsky_feedgen::models::NotFoundErrorCode::UndefinedEndpoint),
        message: Some("Not Found".to_string()),
    };
    Json(path_error)
}

#[catch(422)]
async fn unprocessable_entity() -> Json<rsky_feedgen::models::ValidationErrorMessageResponse> {
    let validation_error = rsky_feedgen::models::ValidationErrorMessageResponse {
        code: Some(rsky_feedgen::models::ErrorCode::ValidationError),
        message: Some(
            "The request was well-formed but was unable to be followed due to semantic errors."
                .to_string(),
        ),
    };
    Json(validation_error)
}

#[catch(400)]
async fn bad_request() -> Json<rsky_feedgen::models::ValidationErrorMessageResponse> {
    let validation_error = rsky_feedgen::models::ValidationErrorMessageResponse {
        code: Some(rsky_feedgen::models::ErrorCode::ValidationError),
        message: Some("The request was improperly formed.".to_string()),
    };
    Json(validation_error)
}

#[catch(401)]
async fn unauthorized() -> Json<rsky_feedgen::models::InternalErrorMessageResponse> {
    let internal_error = rsky_feedgen::models::InternalErrorMessageResponse {
        code: Some(rsky_feedgen::models::InternalErrorCode::Unavailable),
        message: Some("Request could not be processed.".to_string()),
    };
    Json(internal_error)
}

#[catch(default)]
async fn default_catcher() -> Json<rsky_feedgen::models::InternalErrorMessageResponse> {
    let internal_error = rsky_feedgen::models::InternalErrorMessageResponse {
        code: Some(rsky_feedgen::models::InternalErrorCode::InternalError),
        message: Some("Internal error.".to_string()),
    };
    Json(internal_error)
}

/// Catches all OPTION requests in order to get the CORS related Fairing triggered.
#[options("/<_..>")]
async fn all_options() {
    /* Intentionally left empty */
}

#[rocket::async_trait]
impl Fairing for CORS {
    fn info(&self) -> Info {
        Info {
            name: "Add CORS headers to responses",
            kind: Kind::Response,
        }
    }

    async fn on_response<'r>(&self, _request: &'r Request<'_>, response: &mut Response<'r>) {
        response.set_header(Header::new("Access-Control-Allow-Origin", "*"));
        response.set_header(Header::new(
            "Access-Control-Allow-Methods",
            "POST, GET, PATCH, OPTIONS, DELETE",
        ));
        response.set_header(Header::new("Access-Control-Allow-Headers", "*"));
        response.set_header(Header::new("Access-Control-Allow-Credentials", "true"));
    }
}

#[launch]
fn rocket() -> _ {
    dotenv().ok();

    let write_database_url = env::var("DATABASE_URL").unwrap_or("".into());
    let read_database_url = env::var("READ_REPLICA_URL").unwrap_or("".into());

    let write_db: Map<_, Value> = map! {
        "url" => write_database_url.into(),
        "pool_size" => 20.into(),
        "timeout" => 30.into(),
    };

    let read_db: Map<_, Value> = map! {
        "url" => read_database_url.into(),
        "pool_size" => 20.into(),
        "timeout" => 30.into(),
    };

    let figment = rocket::Config::figment().merge((
        "databases",
        map!["pg_read_replica" => read_db, "pg_db" => write_db],
    ));

    rocket::custom(figment)
        .mount(
            "/",
            routes![
                index,
                queue_creation,
                queue_deletion,
                well_known,
                get_cursor,
                update_cursor,
                all_options
            ],
        )
        .register(
            "/",
            catchers![
                default_catcher,
                unprocessable_entity,
                bad_request,
                not_found,
                unauthorized
            ],
        )
        .attach(CORS)
        .attach(WriteDbConn::fairing())
        .attach(ReadReplicaConn::fairing())
}
