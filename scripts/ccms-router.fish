# 路由模式下开 shell 自动确保 router 在跑。
# 安装：cp scripts/ccms-router.fish ~/.config/fish/conf.d/
# 判定依据是 Claude 设置里的占位 token，而不是 0.1 的 state.json。
if type -q ccms
    if test -f ~/.claude/settings.json
        if grep -qs 'ccms-router-local' ~/.claude/settings.json
            ccms router ensure >/dev/null 2>&1 &
        end
    end
end
