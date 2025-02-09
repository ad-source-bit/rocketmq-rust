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
#![allow(unused_variables)]

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use cheetah_string::CheetahString;
use rocketmq_common::common::broker::broker_config::BrokerConfig;
use rocketmq_common::common::key_builder::POP_ORDER_REVIVE_QUEUE;
use rocketmq_common::common::message::message_decoder;
use rocketmq_common::common::message::message_ext_broker_inner::MessageExtBrokerInner;
use rocketmq_common::common::message::MessageConst;
use rocketmq_common::common::message::MessageTrait;
use rocketmq_common::common::pop_ack_constants::PopAckConstants;
use rocketmq_common::common::FAQUrl;
use rocketmq_common::TimeUtils::get_current_millis;
use rocketmq_remoting::code::request_code::RequestCode;
use rocketmq_remoting::code::response_code::ResponseCode;
use rocketmq_remoting::net::channel::Channel;
use rocketmq_remoting::protocol::body::batch_ack::BatchAck;
use rocketmq_remoting::protocol::body::batch_ack_message_request_body::BatchAckMessageRequestBody;
use rocketmq_remoting::protocol::header::ack_message_request_header::AckMessageRequestHeader;
use rocketmq_remoting::protocol::header::extra_info_util::ExtraInfoUtil;
use rocketmq_remoting::protocol::remoting_command::RemotingCommand;
use rocketmq_remoting::protocol::RemotingDeserializable;
use rocketmq_remoting::protocol::RemotingSerializable;
use rocketmq_remoting::runtime::connection_handler_context::ConnectionHandlerContext;
use rocketmq_rust::ArcMut;
use rocketmq_store::base::message_status_enum::PutMessageStatus;
use rocketmq_store::log_file::MessageStore;
use rocketmq_store::pop::ack_msg::AckMsg;
use rocketmq_store::pop::batch_ack_msg::BatchAckMsg;
use rocketmq_store::pop::AckMessage;
use tracing::error;

use crate::broker_error::BrokerError::BrokerCommonError;
use crate::broker_error::BrokerError::BrokerRemotingError;
use crate::failover::escape_bridge::EscapeBridge;
use crate::processor::pop_inflight_message_counter::PopInflightMessageCounter;
use crate::processor::pop_message_processor::PopMessageProcessor;
use crate::processor::processor_service::pop_buffer_merge_service::PopBufferMergeService;
use crate::topic::manager::topic_config_manager::TopicConfigManager;

pub struct AckMessageProcessor<MS> {
    topic_config_manager: TopicConfigManager,
    message_store: ArcMut<MS>,
    pop_buffer_merge_service: ArcMut<PopBufferMergeService>,
    escape_bridge: ArcMut<EscapeBridge<MS>>,
    store_host: SocketAddr,
    pop_inflight_message_counter: Arc<PopInflightMessageCounter>,
}

impl<MS> AckMessageProcessor<MS>
where
    MS: MessageStore,
{
    pub fn new(
        topic_config_manager: TopicConfigManager,
        message_store: ArcMut<MS>,
        escape_bridge: ArcMut<EscapeBridge<MS>>,
        broker_config: Arc<BrokerConfig>,
        pop_inflight_message_counter: Arc<PopInflightMessageCounter>,
        store_host: SocketAddr,
    ) -> AckMessageProcessor<MS> {
        AckMessageProcessor {
            topic_config_manager,
            message_store,
            /* need to implement PopBufferMergeService */
            pop_buffer_merge_service: ArcMut::new(PopBufferMergeService),
            escape_bridge,
            store_host,
            pop_inflight_message_counter,
        }
    }

    pub async fn process_request(
        &mut self,
        channel: Channel,
        ctx: ConnectionHandlerContext,
        request_code: RequestCode,
        request: RemotingCommand,
    ) -> crate::Result<Option<RemotingCommand>> {
        match request_code {
            RequestCode::AckMessage => self.process_ack(channel, ctx, request, true).await,
            RequestCode::BatchAckMessage => {
                self.process_batch_ack(channel, ctx, request, true).await
            }
            _ => Ok(Some(
                RemotingCommand::create_response_command_with_code_remark(
                    ResponseCode::MessageIllegal,
                    format!(
                        "request code not supported, request code: {:?}",
                        request_code
                    ),
                ),
            )),
        }
    }
}

impl<MS> AckMessageProcessor<MS>
where
    MS: MessageStore,
{
    async fn process_ack(
        &mut self,
        channel: Channel,
        _ctx: ConnectionHandlerContext,
        request: RemotingCommand,
        _broker_allow_suspend: bool,
    ) -> crate::Result<Option<RemotingCommand>> {
        let request_header = request
            .decode_command_custom_header::<AckMessageRequestHeader>()
            .map_err(BrokerRemotingError)?;
        let topic_config = self
            .topic_config_manager
            .select_topic_config(&request_header.topic);
        if topic_config.is_none() {
            return Ok(Some(
                RemotingCommand::create_response_command_with_code_remark(
                    ResponseCode::TopicNotExist,
                    format!(
                        "topic[{}] not exist, apply first please! {}",
                        request_header.topic,
                        FAQUrl::suggest_todo(FAQUrl::APPLY_TOPIC_URL)
                    ),
                ),
            ));
        }
        let topic_config = topic_config.unwrap();
        if request_header.queue_id >= topic_config.read_queue_nums as i32
            || request_header.queue_id < 0
        {
            let error_msg = format!(
                "queueId{}] is illegal, topic:[{}] topicConfig.readQueueNums:[{}] consumer:[{}]",
                request_header.queue_id,
                request_header.topic,
                topic_config.read_queue_nums,
                channel.remote_address()
            );
            return Ok(Some(
                RemotingCommand::create_response_command_with_code_remark(
                    ResponseCode::MessageIllegal,
                    error_msg,
                ),
            ));
        }
        let min_offset = self
            .message_store
            .get_min_offset_in_queue(&request_header.topic, request_header.queue_id);
        let max_offset = self
            .message_store
            .get_max_offset_in_queue(&request_header.topic, request_header.queue_id);
        if request_header.offset < min_offset || request_header.offset > max_offset {
            let error_msg = format!(
                "request offset not in queue offset range, request offset: {}, min offset: {}, \
                 max offset: {}",
                request_header.offset, min_offset, max_offset
            );
            return Ok(Some(
                RemotingCommand::create_response_command_with_code_remark(
                    ResponseCode::NoMessage,
                    error_msg,
                ),
            ));
        }
        let mut response = RemotingCommand::create_response_command();
        self.append_ack(Some(request_header), &mut response, None, &channel, None)
            .await;
        Ok(Some(response))
    }

    async fn process_batch_ack(
        &mut self,
        _channel: Channel,
        _ctx: ConnectionHandlerContext,
        request: RemotingCommand,
        _broker_allow_suspend: bool,
    ) -> crate::Result<Option<RemotingCommand>> {
        if request.get_body().is_none() {
            return Ok(Some(RemotingCommand::create_response_command_with_code(
                ResponseCode::NoMessage,
            )));
        }
        let req_body = BatchAckMessageRequestBody::decode(request.get_body().unwrap())
            .map_err(BrokerCommonError)?;
        if req_body.acks.is_empty() {
            return Ok(Some(RemotingCommand::create_response_command_with_code(
                ResponseCode::NoMessage,
            )));
        }
        let mut response = RemotingCommand::create_response_command();
        let broker_name = &req_body.broker_name;
        for ack in req_body.acks {
            self.append_ack(None, &mut response, Some(ack), &_channel, Some(broker_name))
                .await;
        }
        Ok(Some(response))
    }

    async fn append_ack(
        &mut self,
        request_header: Option<AckMessageRequestHeader>,
        response: &mut RemotingCommand,
        batch_ack: Option<BatchAck>,
        channel: &Channel,
        broker_name: Option<&CheetahString>,
    ) {
        //handle single ack
        let (
            consume_group,
            topic,
            qid,
            r_qid,
            start_offset,
            ack_offset,
            pop_time,
            invisible_time,
            ack_count,
            mut ack_msg,
            broker_name,
        ) = if let Some(request_header) = request_header {
            let extra_info =
                ExtraInfoUtil::split(request_header.extra_info.as_str()).unwrap_or_default();
            let broker_name =
                ExtraInfoUtil::get_broker_name(extra_info.as_slice()).unwrap_or_default();
            let consume_group = request_header.consumer_group.clone();
            let topic = request_header.topic.clone();
            let qid = request_header.queue_id;
            let r_qid = ExtraInfoUtil::get_revive_qid(extra_info.as_slice()).unwrap_or_default();
            let start_offset =
                ExtraInfoUtil::get_ck_queue_offset(extra_info.as_slice()).unwrap_or_default();
            let ack_offset = request_header.offset;
            let pop_time = ExtraInfoUtil::get_pop_time(extra_info.as_slice()).unwrap_or_default();
            let invisible_time =
                ExtraInfoUtil::get_invisible_time(extra_info.as_slice()).unwrap_or_default();
            if r_qid == POP_ORDER_REVIVE_QUEUE {
                self.ack_orderly(
                    topic,
                    consume_group,
                    qid,
                    ack_offset,
                    pop_time,
                    invisible_time,
                    channel,
                    response,
                );
                return;
            }
            let ack = AckMsg::default();
            let ack_count = 1;
            (
                consume_group,
                topic,
                qid,
                r_qid,
                start_offset,
                ack_offset,
                pop_time,
                invisible_time,
                ack_count,
                Box::new(ack) as Box<dyn AckMessage + Send>,
                CheetahString::from(broker_name),
            )
        } else {
            //handle batch ack
            let batch_ack = batch_ack.unwrap();
            let consume_group = batch_ack.consumer_group.clone();
            let topic = CheetahString::from(
                ExtraInfoUtil::get_real_topic_with_retry(
                    batch_ack.topic.as_str(),
                    batch_ack.consumer_group.as_str(),
                    batch_ack.retry.as_str(),
                )
                .unwrap_or_default(),
            );
            let qid = batch_ack.queue_id;
            let r_qid = batch_ack.revive_queue_id;
            let start_offset = batch_ack.start_offset;
            let akc_offset = -1;
            let pop_time = batch_ack.pop_time;
            let invisible_time = batch_ack.invisible_time;
            let min_offset = self.message_store.get_min_offset_in_queue(&topic, qid);
            let max_offset = self.message_store.get_max_offset_in_queue(&topic, qid);
            if min_offset == -1 || max_offset == -1 {
                //error!("Illegal topic or queue found when batch ack {:?}", batch_ack);
                return;
            }

            let mut batch_ack_msg = BatchAckMsg::default();

            let bit_set = &batch_ack.bit_set.0;
            for i in bit_set.iter_ones() {
                if i == usize::MAX {
                    break;
                }
                let offset = batch_ack.start_offset + i as i64;
                if offset < min_offset || offset > max_offset {
                    continue;
                }
                if r_qid == POP_ORDER_REVIVE_QUEUE {
                    self.ack_orderly(
                        topic.clone(),
                        consume_group.clone(),
                        qid,
                        offset,
                        pop_time,
                        invisible_time,
                        channel,
                        response,
                    );
                } else {
                    batch_ack_msg.ack_offset_list.push(offset);
                }
            }
            if r_qid == POP_ORDER_REVIVE_QUEUE || batch_ack_msg.ack_offset_list.is_empty() {
                return;
            }
            if r_qid == POP_ORDER_REVIVE_QUEUE || batch_ack_msg.ack_offset_list.is_empty() {
                return;
            }
            let ack_count = batch_ack_msg.ack_offset_list.len();
            //let ack = batch_ack_msg.ack_msg;
            (
                consume_group,
                topic,
                qid,
                r_qid,
                start_offset,
                -1,
                pop_time,
                invisible_time,
                ack_count,
                Box::new(batch_ack_msg) as Box<dyn AckMessage + Send>,
                broker_name.unwrap().clone(),
            )
        };

        //this.brokerController.getBrokerStatsManager().incBrokerAckNums(ackCount);
        //this.brokerController.getBrokerStatsManager().incGroupAckNums(consumeGroup,topic,
        // ackCount);
        ack_msg.set_consumer_group(consume_group.clone());
        ack_msg.set_topic(topic.clone());
        ack_msg.set_queue_id(qid);
        ack_msg.set_start_offset(start_offset);
        ack_msg.set_ack_offset(ack_offset);
        ack_msg.set_pop_time(pop_time);
        ack_msg.set_broker_name(broker_name);
        if self
            .pop_buffer_merge_service
            .add_ack(r_qid, ack_msg.as_ref())
        {
            return;
        }
        let mut inner = MessageExtBrokerInner::default();
        inner.set_topic(topic.clone());
        inner.message_ext_inner.queue_id = qid;
        if let Some(batch_ack) = ack_msg.as_any().downcast_ref::<BatchAckMsg>() {
            inner.set_body(Bytes::from(batch_ack.encode().unwrap()));
            inner.set_tags(CheetahString::from_static_str(
                PopAckConstants::BATCH_ACK_TAG,
            ));
            inner.put_property(
                CheetahString::from_static_str(
                    MessageConst::PROPERTY_UNIQ_CLIENT_MESSAGE_ID_KEYIDX,
                ),
                CheetahString::from(PopMessageProcessor::gen_batch_ack_unique_id(batch_ack)),
            );
        } else if let Some(ack_msg) = ack_msg.as_any().downcast_ref::<AckMsg>() {
            inner.set_body(Bytes::from(ack_msg.encode().unwrap()));
            inner.set_tags(CheetahString::from_static_str(PopAckConstants::ACK_TAG));
            inner.put_property(
                CheetahString::from_static_str(
                    MessageConst::PROPERTY_UNIQ_CLIENT_MESSAGE_ID_KEYIDX,
                ),
                CheetahString::from(PopMessageProcessor::gen_ack_unique_id(
                    ack_msg as &dyn AckMessage,
                )),
            );
        }
        inner.message_ext_inner.born_timestamp = get_current_millis() as i64;
        inner.message_ext_inner.store_host = self.store_host;
        inner.set_delay_time_ms((pop_time + invisible_time) as u64);
        inner.put_property(
            CheetahString::from_static_str(MessageConst::PROPERTY_UNIQ_CLIENT_MESSAGE_ID_KEYIDX),
            CheetahString::from(PopMessageProcessor::gen_ack_unique_id(ack_msg.as_ref())),
        );
        inner.properties_string =
            message_decoder::message_properties_to_string(inner.get_properties());
        let put_message_result = self
            .escape_bridge
            .put_message_to_specific_queue(inner)
            .await;
        match put_message_result.put_message_status() {
            PutMessageStatus::PutOk
            | PutMessageStatus::FlushDiskTimeout
            | PutMessageStatus::FlushSlaveTimeout
            | PutMessageStatus::SlaveNotAvailable => {}
            _ => {
                error!(
                    "put ack msg error:{:?}",
                    put_message_result.put_message_status()
                );
            }
        }
        self.pop_inflight_message_counter
            .decrement_in_flight_message_num(
                &topic,
                &consume_group,
                pop_time,
                qid,
                ack_count as i64,
            );
    }

    fn ack_orderly(
        &mut self,
        topic: CheetahString,
        consume_group: CheetahString,
        q_id: i32,
        ack_offset: i64,
        pop_time: i64,
        invisible_time: i64,
        channel: &Channel,
        response: &mut RemotingCommand,
    ) {
        unimplemented!("ack_orderly")
    }
}
