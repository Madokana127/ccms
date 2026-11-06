# 路由模式下开 shell 自动确保 router 在跑。
# 安装：cp scripts/ccms-router.fish ~/.config/fish/conf.d/
# 判定依据是 Claude 设置里的本机 base_url，而不是 0.1 的 state.json，
# 也不再依赖固定的占位 token（token 每个 CCMS_HOME 随机生成）。
if type -q ccms
    if test -f ~/.claude/settings.json
        if grep -qs 'ANTHROPIC_BASE_URL.*127\.0\.0\.1' ~/.claude/settings.json
            ccms --router --ensure >/dev/null 2>&1 &
        end
    end
end
