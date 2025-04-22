view_json() {
    sed 's/| //' | awk '/^[[:space:]]*-+$/ { exit } /^[[:print:]]*$/ { print }' | jq .
}
