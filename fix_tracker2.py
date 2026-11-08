with open('c:\\Users\\DAVID UZOMAH\\Desktop\\LogicLabs\\AgentTrace\\.github\\workflows\\test-failure-tracker.yml', 'rb') as f:
    content = f.read()

# Fix the printf line - replace the broken format string
content = content.replace(
    b"printf '%s\r\n' \"${FAILED_TESTS[@]}\" | sort -u > failed_tests.txt\r\n",
    b"printf '%s\\n' \"${FAILED_TESTS[@]}\" | sort -u > failed_tests.txt\n"
)

with open('c:\\Users\\DAVID UZOMAH\\Desktop\\LogicLabs\\AgentTrace\\.github\\workflows\\test-failure-tracker.yml', 'wb') as f:
    f.write(content)

print('Fixed')