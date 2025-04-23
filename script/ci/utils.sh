parse_args() {
    local descriptor="$1"
    shift
    local args=("$@")

    IFS=',' read -r -a pairs <<< "$descriptor"

    for pair in "${pairs[@]}"; do
        IFS=':' read -r arg_name var_name <<< "$pair"

        for ((i=0; i<${#args[@]}; i+=2)); do
            if [[ "${args[i]}" == "$arg_name" ]]; then
                eval "$var_name=\"\${args[i+1]}\""
                break
            fi
        done
    done
}
