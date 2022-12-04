use hmac::{Hmac, Mac};
use md5::Md5;
use tracing::{debug, error, info, instrument};
pub mod payloads;
use payloads::*;
use reqwest::{self, StatusCode};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing_unwrap::{OptionExt, ResultExt};

// HMAC MD5
type HmacMd5 = Hmac<Md5>;
const UNDEFINED_PRIVATE_KEY: &str = "withoutloginkey";
const SOAP_DOMAIN: &str = "http://purenetworks.com/HNAP1/";

/// Uppercase the hash resulting from running HMAC-MD5 with key on data
pub fn hex_hmac_md5(key: &[u8], data: &[u8]) -> String {
    let mut mac = HmacMd5::new_from_slice(key).unwrap();
    mac.update(data);
    let result = mac.finalize().into_bytes();
    hex::encode_upper(result)
}

#[derive(Default, Debug)]
pub struct SOAPClient {
    client: reqwest::Client,
    endpoint: String,
    private_key: String,
    cookie: String,
}

impl SOAPClient {
    pub fn new(endpoint: String, accept_invalid_certs: bool) -> SOAPClient {
        SOAPClient {
            client: reqwest::Client::builder()
                .danger_accept_invalid_certs(accept_invalid_certs)
                .build()
                .unwrap(),
            endpoint,
            private_key: UNDEFINED_PRIVATE_KEY.to_string(),
            cookie: "".to_string(),
        }
    }

    async fn send_soap_action<T>(
        &mut self,
        action: &str,
        additional_params: &HashMap<&str, &str>,
    ) -> Result<T, &str>
    where
        T: DeserializeOwned + std::fmt::Debug + HasResult,
    {
        let current_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
            .to_string();
        let soap_action_uri = format!(r#"{}{}"#, SOAP_DOMAIN, action);
        let message = current_time.to_owned() + &soap_action_uri;

        let auth =
            hex_hmac_md5(self.private_key.as_bytes(), message.as_bytes()) + " " + &current_time;
        // debug!("{}", auth);

        // additional_params gets nested under the action for no reason
        let mut nested_additional_params = HashMap::new();
        nested_additional_params.insert(action, additional_params);
        debug!("Sending payload: {:?}", nested_additional_params);

        // create the request
        let req = self
            .client
            .post(&self.endpoint)
            .header("SOAPAction", SOAP_DOMAIN.to_owned() + action)
            .header("HNAP_AUTH", auth)
            .header(
                "Cookie",
                format!(
                    "Secure; uid={}; PrivateKey={}",
                    self.cookie, self.private_key
                ),
            )
            .json(&nested_additional_params);
        debug!("Sending request: {:?}", req);

        // fire off the request
        let res = req.send().await.unwrap_or_log();

        // serialize to Value so we can print out the whole payload first
        let serialized_json: serde_json::Value = match res.status() {
            StatusCode::OK => res.json().await.unwrap_or_log(),
            _ => {
                error!("{:?}", res);
                return Err("Modem did not return 200 OK, dumped response to log");
            }
        };
        debug!("JSON reply from modem: {:?}", serialized_json);
        // rebind here to the concrete type so that we can return the right type
        let mut serialized_json =
            serde_json::value::from_value::<HashMap<String, T>>(serialized_json).unwrap_or_log();

        // serialized_json[action + "Response"][action + "Result"] will tell us if bad JSON returned
        // unclear why they couldn't just 400/500 that, but whatever.
        let unwrapped_json = serialized_json
            .remove(&(action.to_owned() + "Response"))
            .unwrap_or_log();
        match unwrapped_json.get_result().as_str() {
            "ERROR" => {
                error!("{:?}", serialized_json);
                Err("JSON said there was an ERROR, aborting")
            }
            _ => Ok(unwrapped_json),
        }
    }

    #[instrument]
    async fn login_with_challenge(
        &mut self,
        username: &str,
        password: &str,
        public_key: &str,
        challenge: &str,
        cookie: &str,
    ) -> Result<LoginWithChallengeResponse, &str> {
        self.cookie = cookie.to_string();
        // compute the private key, which is HMAC(pubkey + password, challenge)
        let private_key = hex_hmac_md5(
            (public_key.to_owned() + password).as_bytes(),
            challenge.as_bytes(),
        );
        // set our private key, looks important
        self.private_key = private_key;
        debug!("Private key: {}", &self.private_key);

        // the login password is HMAC(PRIV_KEY, CHALLENGE)
        let login_password = hex_hmac_md5(self.private_key.as_bytes(), challenge.as_bytes());

        // this second login attempt is the real login attempt
        let request_hashmap: HashMap<&str, &str> = HashMap::from([
            ("Action", "login"),
            ("Username", username),
            ("LoginPassword", &login_password),
            ("Captcha", ""),
            ("PrivateLogin", "LoginPassword"),
        ]);

        let login_response: LoginWithChallengeResponse = self
            .send_soap_action("Login", &request_hashmap)
            .await
            .expect("Unable to login with calculated credentials");

        match login_response.get_result().as_str() {
            "OK_CHANGED" => Err("May need to reset login settings, idk haven't actually hit this"),
            "FAILED" => Err("Username or password error"),
            "LOCKUP" => Err("Max number of login attempts reached"),
            "REBOOT" => Err("Account locked, reboot required to re-enable account"),
            "OK" => Ok(login_response),
            _ => Err("Unknown response from modem"),
        }
    }

    #[instrument]
    pub async fn login(&mut self, username: &str, password: &str) {
        // the first login request has an Action: request and retrieves the challenge + public key
        let request_hashmap: HashMap<&str, &str> =
            HashMap::from([("Action", "request"), ("Username", username)]);

        // challenge and pubkey should be contained here
        let response: LoginResponse = self
            .send_soap_action("Login", &request_hashmap)
            .await
            .expect("Unable to get challenge and pubkey from modem");

        self.login_with_challenge(
            username,
            password,
            &response.public_key,
            &response.challenge,
            &response.cookie,
        )
        .await
        .unwrap_or_log();
    }

    #[instrument]
    pub async fn metrics(&mut self) -> GetMultipleHNAPsMetricsResponse {
        let request_hashmap: HashMap<&str, &str> = HashMap::from([
            ("GetArrisDeviceStatus", ""),
            ("GetArrisRegisterInfo", ""),
            // ("GetArrisRegisterStatus", ""), // ok we don't really care
            ("GetCustomerStatusStartupSequence", ""),
            ("GetCustomerStatusConnectionInfo", ""),
            ("GetCustomerStatusDownstreamChannelInfo", ""),
            ("GetCustomerStatusUpstreamChannelInfo", ""),
        ]);
        let response: GetMultipleHNAPsMetricsResponse = self
            .send_soap_action("GetMultipleHNAPs", &request_hashmap)
            .await
            .expect("Unable to get metrics from modem");

        info!("{:#?}", response);
        response
    }

    #[instrument]
    pub async fn logs(&mut self) -> GetMultipleHNAPsLogsResponse {
        let request_hashmap: HashMap<&str, &str> = HashMap::from([
            ("GetCustomerStatusLog", ""),
            ("GetCustomerStatusLogXXX", ""), // this just returns `XXX`, useless
        ]);
        let response: GetMultipleHNAPsLogsResponse = self
            .send_soap_action("GetMultipleHNAPs", &request_hashmap)
            .await
            .expect("Unable to get logs from modem");

        info!("{:#?}", response);
        response
    }
}
