use serde_json::{json, Value};
use templar_gateway_methods_spec::dao::AddProposal;

use super::request_fingerprint;

fn request(kind: &Value) -> Value {
    json!({
        "dao_id": "dao.near",
        "proposal": {"description": "Policy update", "kind": kind}
    })
}

fn vote_policy() -> Value {
    json!({"weight_kind": "RoleWeight", "quorum": "0", "threshold": [1, 2]})
}

fn role(group: &[&str], permissions: &[&str]) -> Value {
    json!({
        "name": "council",
        "kind": {"Group": group},
        "permissions": permissions,
        "vote_policy": {"call": vote_policy()}
    })
}

fn policy(role: &Value) -> Value {
    json!({
        "roles": [role, {
            "name": "everyone", "kind": "Everyone",
            "permissions": ["*:AddProposal"], "vote_policy": {}
        }],
        "default_vote_policy": vote_policy(),
        "proposal_bond": "1", "proposal_period": "1000",
        "bounty_bond": "2", "bounty_forgiveness_period": "2000"
    })
}

fn permutations(values: [&str; 3]) -> [[&str; 3]; 6] {
    let [a, b, c] = values;
    [
        [a, b, c],
        [a, c, b],
        [b, a, c],
        [b, c, a],
        [c, a, b],
        [c, b, a],
    ]
}

#[test]
fn policy_set_permutations_have_exact_stable_wire_and_fingerprint() {
    let group = ["alice.near", "bob.near", "carol.near"];
    let permissions = ["*:AddProposal", "*:VoteApprove", "*:VoteReject"];
    let canonical_role = role(&group, &permissions);
    for full_policy in [false, true] {
        let kind = |role: &Value| {
            if full_policy {
                json!({"ChangePolicy": {"policy": policy(role)}})
            } else {
                json!({"ChangePolicyAddOrUpdateRole": {"role": role}})
            }
        };
        let expected = request(&kind(&canonical_role));
        // A fixed JSON oracle, not another randomized native HashSet serializer.
        let expected_fingerprint = request_fingerprint("dao.addProposal", &expected).unwrap();
        for members in permutations(group) {
            for allowed in permutations(permissions) {
                let input =
                    serde_json::to_string(&request(&kind(&role(&members, &allowed)))).unwrap();
                // Decode each retry independently; cloning shares the HashSet's seed.
                for _ in 0..2 {
                    let decoded: AddProposal = serde_json::from_str(&input).unwrap();
                    assert_eq!(
                        request_fingerprint("dao.addProposal", &decoded).unwrap(),
                        expected_fingerprint,
                        "set permutations must have the canonical request fingerprint"
                    );
                    assert_eq!(
                        serde_json::to_value(&decoded).unwrap(),
                        expected,
                        "group members and permissions must be lexically ordered"
                    );
                }
            }
        }
    }
}

#[test]
fn role_and_action_order_remain_part_of_request_identity() {
    let mut original = request(&json!({"ChangePolicy": {"policy": policy(&role(
        &["alice.near"], &["*:VoteApprove"]
    ))}}));
    let before: AddProposal = serde_json::from_value(original.clone()).unwrap();
    original["proposal"]["kind"]["ChangePolicy"]["policy"]["roles"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let after: AddProposal = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(serde_json::to_value(&after).unwrap(), original);
    assert_ne!(
        request_fingerprint("dao.addProposal", &before).unwrap(),
        request_fingerprint("dao.addProposal", &after).unwrap()
    );

    let mut original = request(&json!({"FunctionCall": {
        "receiver_id": "target.near", "actions": [
            {"method_name": "first", "args": "AAEC/w==", "deposit": "1", "gas": "10"},
            {"method_name": "second", "args": "AwIBAA==", "deposit": "2", "gas": "20"}
        ]
    }}));
    let before: AddProposal = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(serde_json::to_value(&before).unwrap(), original);
    original["proposal"]["kind"]["FunctionCall"]["actions"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let after: AddProposal = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(serde_json::to_value(&after).unwrap(), original);
    assert_ne!(
        request_fingerprint("dao.addProposal", &before).unwrap(),
        request_fingerprint("dao.addProposal", &after).unwrap()
    );
}

#[test]
fn native_non_set_policy_forms_are_unchanged() {
    for kind in [
        json!({"ChangePolicy": {"policy": ["bob.near", "alice.near"]}}),
        json!({"ChangePolicyAddOrUpdateRole": {"role": {
            "name": "holder", "kind": {"Member": "100"},
            "permissions": [], "vote_policy": {}
        }}}),
        json!({"ChangePolicyAddOrUpdateRole": {"role": {
            "name": "everyone", "kind": "Everyone",
            "permissions": [], "vote_policy": {}
        }}}),
        json!("Vote"),
    ] {
        let expected = request(&kind);
        let decoded: AddProposal = serde_json::from_value(expected.clone()).unwrap();
        assert_eq!(serde_json::to_value(&decoded).unwrap(), expected);
        assert_eq!(
            request_fingerprint("dao.addProposal", &decoded).unwrap(),
            request_fingerprint("dao.addProposal", &expected).unwrap()
        );
    }
}
