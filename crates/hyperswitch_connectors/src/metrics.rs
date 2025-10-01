//! Metrics interface

use router_env::{counter_metric, global_meter, histogram_metric_f64};

global_meter!(GLOBAL_METER, "ROUTER_API");

counter_metric!(CONNECTOR_RESPONSE_DESERIALIZATION_FAILURE, GLOBAL_METER);

// NATS Bridge metrics
histogram_metric_f64!(
    NATS_REQUEST_DURATION_SECONDS,
    GLOBAL_METER,
    "Time taken for NATS request-reply operations in seconds"
);
counter_metric!(NATS_TIMEOUT_COUNT, GLOBAL_METER);
counter_metric!(NATS_JETSTREAM_PUBLISH_ERRORS, GLOBAL_METER);
counter_metric!(NATS_WORKER_RESPONSE_SUCCESS, GLOBAL_METER);
counter_metric!(NATS_JETSTREAM_ACK_SUCCESS, GLOBAL_METER);
