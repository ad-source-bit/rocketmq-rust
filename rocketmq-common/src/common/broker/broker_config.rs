/*
 * Licensed to the Apache Software Foundation (ASF) under one or more
 * contributor license agreements.  See the NOTICE file distributed with
 * this work for additional information regarding copyright ownership.
 * The ASF licenses this file to You under the Apache License, Version 2.0
 * (the "License"); you may not use this file except in compliance with
 * the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use serde::Deserialize;

use crate::common::{
    constant::PermName, mix_all, server::config::ServerConfig, topic::TopicValidator,
};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokerIdentity {
    pub broker_name: String,
    pub broker_cluster_name: String,
    pub broker_id: u64,
    pub is_broker_container: bool,
    pub is_in_broker_container: bool,
}

impl BrokerIdentity {
    pub fn new() -> Self {
        let broker_name = default_broker_name();
        let broker_cluster_name = String::from("DefaultCluster");
        let broker_id = mix_all::MASTER_ID;
        let is_broker_container = false;

        BrokerIdentity {
            broker_name,
            broker_cluster_name,
            broker_id,
            is_broker_container,
            is_in_broker_container: false,
        }
    }

    fn new_with_container(is_broker_container: bool) -> Self {
        let mut identity = BrokerIdentity::new();
        identity.is_broker_container = is_broker_container;
        identity
    }

    fn new_with_params(broker_cluster_name: String, broker_name: String, broker_id: u64) -> Self {
        BrokerIdentity {
            broker_name,
            broker_cluster_name,
            broker_id,
            is_broker_container: false,
            is_in_broker_container: false,
        }
    }

    fn new_with_container_params(
        broker_cluster_name: String,
        broker_name: String,
        broker_id: u64,
        is_in_broker_container: bool,
    ) -> Self {
        BrokerIdentity {
            broker_name,
            broker_cluster_name,
            broker_id,
            is_broker_container: true,
            is_in_broker_container,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrokerConfig {
    pub broker_identity: BrokerIdentity,

    pub topic_config: TopicConfig,

    pub topic_queue_config: TopicQueueConfig,

    pub timer_wheel_config: TimerWheelConfig,

    pub broker_server_config: ServerConfig,

    pub broker_ip1: String,
    pub broker_ip2: Option<String>,
    pub listen_port: u32,
    pub trace_topic_enable: bool,
    pub msg_trace_topic_name: String,
    pub enable_controller_mode: bool,
    pub broker_name: String,
    pub region_id: String,
    pub trace_on: bool,
    pub broker_permission: i8,
    pub async_send_enable: bool,
    pub store_path_root_dir: String,
    pub enable_split_registration: bool,
    pub split_registration_size: i32,
    pub register_broker_timeout_mills: i32,
    pub is_in_broker_container: bool,
    pub commercial_size_per_msg: i32,
    pub recover_concurrently: bool,
}

impl Default for BrokerConfig {
    fn default() -> Self {
        let broker_identity = BrokerIdentity::new();
        let local_ip = local_ip_address::local_ip().unwrap();
        let broker_ip1 = local_ip.to_string();
        let broker_ip2 = Some(local_ip.to_string());
        let listen_port = 10911;

        BrokerConfig {
            broker_identity,
            topic_config: TopicConfig::default(),
            topic_queue_config: TopicQueueConfig::default(),
            timer_wheel_config: TimerWheelConfig::default(),
            broker_server_config: Default::default(),
            broker_ip1,
            broker_ip2,
            listen_port,
            trace_topic_enable: false,
            msg_trace_topic_name: TopicValidator::RMQ_SYS_TRACE_TOPIC.to_string(),
            enable_controller_mode: false,
            broker_name: "".to_string(),
            region_id: mix_all::DEFAULT_TRACE_REGION_ID.to_string(),
            trace_on: true,
            broker_permission: PermName::PERM_WRITE | PermName::PERM_READ,
            async_send_enable: false,
            store_path_root_dir: dirs::home_dir()
                .unwrap()
                .join("store")
                .to_string_lossy()
                .into_owned(),
            enable_split_registration: false,
            split_registration_size: 800,
            register_broker_timeout_mills: 24000,
            is_in_broker_container: false,
            commercial_size_per_msg: 4 * 1024,
            recover_concurrently: false,
        }
    }
}

impl BrokerConfig {
    pub fn broker_name(&self) -> String {
        self.broker_name.clone()
    }

    pub fn broker_ip1(&self) -> String {
        self.broker_ip1.clone()
    }

    pub fn broker_ip2(&self) -> Option<String> {
        self.broker_ip2.clone()
    }

    pub fn listen_port(&self) -> u32 {
        self.listen_port
    }

    pub fn trace_topic_enable(&self) -> bool {
        self.trace_topic_enable
    }

    pub fn broker_server_config(&self) -> &ServerConfig {
        &self.broker_server_config
    }

    pub fn region_id(&self) -> String {
        self.region_id.clone()
    }

    pub fn broker_permission(&self) -> i8 {
        self.broker_permission
    }
}

fn default_broker_name() -> String {
    // Implement logic to obtain default broker name
    // For example, use local hostname
    // ...

    // Placeholder value for demonstration
    String::from("DefaultBrokerName")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicConfig {
    pub auto_create_topic_enable: bool,
    pub cluster_topic_enable: bool,
    pub broker_topic_enable: bool,
}

impl Default for TopicConfig {
    fn default() -> Self {
        TopicConfig {
            auto_create_topic_enable: true,
            cluster_topic_enable: true,
            broker_topic_enable: true,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicQueueConfig {
    pub default_topic_queue_nums: u32,
}

impl Default for TopicQueueConfig {
    fn default() -> Self {
        TopicQueueConfig {
            default_topic_queue_nums: 8,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TimerWheelConfig {
    pub timer_wheel_enable: bool,
}
