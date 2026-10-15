#!/bin/bash
# Prepare the collector's directories; existing configuration and batches remain.
set -euo pipefail
umask 077

config_dir=/etc/tapo-telemetry
state_dir=/var/lib/tapo-telemetry
spool_dir=$state_dir/spool

fail() {
    printf 'Error: %s\n' "$1" >&2
    return 1
}

check_path() {
    local path=$1
    while [[ $path != / ]]; do
        if [[ -L $path ]]; then
            fail "Unsafe symlink in $1"
        fi
        path=$(dirname -- "$path")
    done
}

prepare_directories() {
    [[ $(id -u) == 0 ]] || fail 'Run this script with sudo on the automation box.'
    local path name uid gid group_name group_password group_gid
    for path in "$config_dir" "$state_dir" "$spool_dir"; do
        check_path "$path"
        [[ ! -e $path || -d $path ]] || fail "Directory required: $path"
    done
    for name in collector.json mariadb.json credentials.json database-credentials.json; do
        path=$config_dir/$name
        check_path "$path"
        [[ ! -e $path || -f $path ]] || fail "Regular file required: $path"
    done
    if [[ -d $spool_dir ]]; then
        path=$(find -P "$spool_dir" ! -type d ! -type f -print -quit)
        [[ -z $path ]] || fail 'Unsafe entry in spool; inspect it before changing permissions.'
    fi

    if ! id -u tapoagent >/dev/null 2>&1; then
        useradd --system --user-group --home-dir "$state_dir" \
            --no-create-home --shell /usr/sbin/nologin tapoagent
    fi
    uid=$(id -u tapoagent)
    gid=$(id -g tapoagent)
    IFS=: read -r group_name group_password group_gid < <(getent group tapoagent)
    [[ $uid != 0 && $gid != 0 && $group_gid == "$gid" ]] \
        || fail 'tapoagent must be unprivileged with primary group tapoagent.'

    mkdir -p -- "$config_dir" "$spool_dir"
    chown --no-dereference tapoagent:tapoagent -- "$config_dir" "$state_dir" "$spool_dir"
    chmod 700 -- "$config_dir" "$state_dir" "$spool_dir"
    for name in collector.json mariadb.json credentials.json database-credentials.json; do
        path=$config_dir/$name
        if [[ -f $path ]]; then
            chown --no-dereference tapoagent:tapoagent -- "$path"
            chmod 600 -- "$path"
        fi
    done
    find -P "$spool_dir" -type d -exec chown --no-dereference tapoagent:tapoagent -- {} +
    find -P "$spool_dir" -type d -exec chmod 700 -- {} +
    find -P "$spool_dir" -type f -exec chown --no-dereference tapoagent:tapoagent -- {} +
    find -P "$spool_dir" -type f -exec chmod 600 -- {} +
    printf 'Collector directories and existing files now have private tapoagent permissions.\n'
}

if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
    if [[ $# == 1 && $1 == --help ]]; then
        printf 'Usage: sudo bash install.sh\nCreates collector directories and fixes ownership/permissions.\nStop the collector before running; run again after copying configuration files.\n'
    elif [[ $# != 0 ]]; then
        fail 'This script takes no arguments except --help.'
    else
        prepare_directories
    fi
fi
