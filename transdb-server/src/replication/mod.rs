pub mod proto {
    tonic::include_proto!("transdb.replication");
}

pub mod service;

/// Applicative validation that extends what proto3 cannot enforce in the schema.
/// Returns true if:
/// - every entry has a set operation field, and
/// - for each distinct key, seq values appear in strictly increasing order.
///
/// Callers must call this before passing the request to `replicate_batch`.
pub fn is_valid_replicate_batch_request(request: &proto::ReplicateBatchRequest) -> bool {
    use proto::mutation_entry::Operation;
    use std::collections::HashMap;

    let mut last_seq_per_key: HashMap<&str, u64> = HashMap::new();

    for entry in &request.entries {
        let key = match &entry.operation {
            Some(Operation::Put(op)) => op.key.as_str(),
            Some(Operation::Delete(op)) => op.key.as_str(),
            None => return false,
        };

        let last = last_seq_per_key.entry(key).or_insert(0);
        if entry.seq <= *last {
            return false;
        }
        *last = entry.seq;
    }

    true
}
