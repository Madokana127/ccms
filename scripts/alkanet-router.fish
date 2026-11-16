# 路由模式下开 shell 自动确保 router 在跑。
# 安装：cp scripts/alkanet-router.fish ~/.config/fish/conf.d/
if type -q aknt
    if test -f ~/.claude/settings.json
        if grep -qs 'ANTHROPIC_BASE_URL.*127\.0\.0\.1' ~/.claude/settings.json
            aknt router ensure >/dev/null 2>&1 &
        end
    end
end
