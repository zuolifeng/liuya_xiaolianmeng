on run {daemon_file, agent_file, user, cur_pid, source_dir}

  set unload_service to "launchctl unload -w /Library/LaunchDaemons/com.liuyaxiang.remoteapp_service.plist || true;"

  set kill_others to "pgrep -x '六牙小连萌' | grep -v " & cur_pid & " | xargs kill -9;"

  set copy_files to "rm -rf /Applications/六牙小连萌.app && ditto " & source_dir & " /Applications/六牙小连萌.app && chown -R " & quoted form of user & ":staff /Applications/六牙小连萌.app && xattr -r -d com.apple.quarantine /Applications/六牙小连萌.app;"

  set sh1 to "echo " & quoted form of daemon_file & " > /Library/LaunchDaemons/com.liuyaxiang.remoteapp_service.plist && chown root:wheel /Library/LaunchDaemons/com.liuyaxiang.remoteapp_service.plist;"

  set sh2 to "echo " & quoted form of agent_file & " > /Library/LaunchAgents/com.liuyaxiang.remoteapp_server.plist && chown root:wheel /Library/LaunchAgents/com.liuyaxiang.remoteapp_server.plist;"

  set sh3 to "launchctl load -w /Library/LaunchDaemons/com.liuyaxiang.remoteapp_service.plist;"

  set sh to unload_service & kill_others & copy_files & sh1 & sh2 & sh3

  do shell script sh with prompt "六牙小连萌 wants to update itself" with administrator privileges
end run
