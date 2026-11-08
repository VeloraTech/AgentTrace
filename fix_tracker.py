with open('c:\\Users\\DAVID UZOMAH\\Desktop\\LogicLabs\\AgentTrace\\.github\\workflows\\test-failure-tracker.yml', 'r', encoding='utf-8') as f:
    lines = f.readlines()

for i, line in enumerate(lines):
    if 'printf' in line and 'FAILED_TESTS' in line:
        lines[i] = "          printf '%s\\n' \"${FAILED_TESTS[@]}\" | sort -u > failed_tests.txt\n"
        print(f'Fixed line {i+1}')
        break

with open('c:\\Users\\DAVID UZOMAH\\Desktop\\LogicLabs\\AgentTrace\\.github\\workflows\\test-failure-tracker.yml', 'w', encoding='utf-8') as f:
    f.writelines(lines)

print('Done')