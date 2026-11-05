# gjc-connect — Gajae-Code 사내 모델 연결 도구

인터넷이 없는 사내 PC에서 [Gajae-Code](https://github.com/Yeachan-Heo/gajae-code)(`gjc`)를 사내 모델 서버에 연결해 주는 도구입니다.
서버 주소·모델 선택, 연결 확인, gjc 설정 파일 생성, gjc 실행을 한 프로그램으로 처리합니다.

- 지원 환경: **Windows x64**, **macOS Apple Silicon(arm64)**, **macOS Intel(x64)**. Linux는 지원하지 않습니다.
- 접속 대상: vLLM 같은 OpenAI 호환 API 서버(직접 접속) 또는 사내 llm-ops 게이트웨이.
- gjc-connect는 사용자 PC의 `~/.gjc`에 있는 설정·세션을 읽거나 쓰지 않습니다. 모든 설정은 묶음 폴더의 `data/` 안에만 저장됩니다.
  (단, gjc 자체가 내장 라이브러리를 `~/.gjc/natives`에 푸는 것은 막을 수 없습니다. 아래 '홈 폴더에 생기는 파일' 참고)

## 묶음 구성

```text
gjc-connect-<대상>-<버전>/
├─ gjc-connect(.exe)      이 도구
├─ bin/gjc(.exe)          공식 gjc 실행 파일
├─ data/                  설정·프로필 (처음 실행하면 채워짐)
├─ tools/PortableGit/     (Windows) 동봉된 Git for Windows(PortableGit) — Git이 없는 PC에서도 Bash 도구 동작.
│                         다른 Git을 쓰려면 GJC_BASH_PATH 지정
├─ licenses/              gjc 및 Git for Windows(GPLv2) 라이선스·고지
├─ bundle.json            묶음 버전·체크섬 정보
└─ README.md
```

## 설치와 첫 실행 (Windows / Mac 동일)

1. 반입받은 묶음(`.zip` 또는 `.tar.gz`)의 SHA256을 `SHA256SUMS`와 대조합니다.
2. **쓰기 가능한 폴더**에 묶음을 풉니다. (예: `C:\Tools\gjc-connect`, `~/tools/gjc-connect`)
   `data/`에 설정이 저장되므로 읽기 전용 위치에 풀면 안 됩니다.
3. 터미널(PowerShell 또는 macOS 터미널)에서 그 폴더로 이동해 한 줄을 실행합니다.

   ```powershell
   cd C:\Tools\gjc-connect
   .\gjc-connect
   ```

   ```bash
   cd ~/tools/gjc-connect
   ./gjc-connect
   ```

4. 화면 안내를 따릅니다.
   - `[1/3]` 운영체제와 묶음 안의 gjc를 확인합니다.
   - `[2/3]` 설정 이름, 접속 방식(직접/게이트웨이), 서버 주소, 인증, 모델을 입력합니다.
     서버에서 모델 목록을 받아 올 수 있으면 화살표로 고를 수 있습니다.
   - `[3/3]` 서버에 실제로 연결해 봅니다. 성공하면 현재 설정으로 저장되고 바로 gjc를 실행할 수 있습니다.

macOS에서 "확인되지 않은 개발자" 경고가 나오면 묶음 폴더에서 한 번 실행합니다.

```bash
xattr -dr com.apple.quarantine .
```

## 명령어

| 명령 | 설명 |
| --- | --- |
| `gjc-connect` | 첫 실행이면 설정 마법사, 설정이 있으면 메뉴(실행 / 설정 전환 / 새 설정 추가 / 종료) |
| `gjc-connect run [프로젝트폴더]` | 현재 설정으로 gjc 실행. 폴더를 생략하면 현재 폴더에서 실행 |
| `gjc-connect switch` | 저장된 설정을 화살표로 골라 전환 (연결 확인에 성공해야 전환됨) |
| `gjc-connect add` | 새 서버·모델 설정 추가 |
| `gjc-connect list` | 저장된 설정 목록 |
| `gjc-connect remove <이름>` | 설정 삭제 (`-y`로 확인 생략) |
| `gjc-connect test [이름]` | 연결 확인 (이름을 생략하면 현재 설정) |

작업 폴더는 설치 폴더가 아닌 **프로젝트 폴더**여야 합니다. 설치 폴더에서 `run`을 실행하면 프로젝트 폴더를 물어봅니다.

## 연결 확인 결과

```text
성공: 모델이 응답했습니다
  설정 이름: 사내-qwen
  모델: qwen3-coder-next-fp8
  응답 시간: 1.8초
```

```text
실패: 서버에 연결할 수 없습니다
  원인: ...
  해결: 사내망·VPN 연결과 서버 주소를 확인하세요
```

### 모델 목록(`/models`)을 제공하지 않는 서버·게이트웨이

연결 확인은 먼저 `GET {주소}/models`로 모델 존재 여부와 컨텍스트 길이(`max_model_len`)를 확인하고,
이어서 `POST {주소}/chat/completions`로 짧은 요청을 보냅니다. 일부 게이트웨이는 `/models`에 **404 또는 405**로 답합니다.
이 경우(직접·게이트웨이 접속 모두) 오류로 처리하지 않고 모델 목록 확인과 컨텍스트 길이 감지를 건너뛴 뒤,
**채팅 요청 결과만으로** 성공/실패를 판단합니다.

- 설정 입력 중에는 모델 목록 선택 없이 바로 모델 이름을 직접 입력하는 단계로 넘어갑니다. 게이트웨이 관리자에게 받은 모델 이름을 그대로 입력하세요.
- 성공하면 아래 한 줄이 추가로 표시되고, 컨텍스트 길이는 기본값(65536)을 사용합니다.

  ```text
    모델 목록  : 서버가 제공하지 않음 (채팅 요청으로 확인)
  ```

- 채팅 요청이 401/403이면 `API 키가 맞지 않습니다`, 모델이 없다는 404면 `서버에 이 모델이 없습니다`, 그 밖의 예상 밖 응답이면 `서버 응답을 이해할 수 없습니다`로 분류됩니다.
- `/models`가 401/403이면 목록 제공 여부와 무관하게 인증 실패로 처리합니다.

실패 후에는 `연결 정보 다시 입력` / `다시 시도` / `저장하고 나중에 확인` / `종료` 중에서 고릅니다.
`저장하고 나중에 확인`은 설정만 저장하고 현재 설정은 바꾸지 않습니다.

## 보안과 저장 위치

- **API 키**는 `models.yml`에 쓰지 않습니다. Windows 자격 증명 관리자 / macOS 키체인에 저장하고,
  gjc를 실행할 때만 환경변수로 전달합니다. 키체인을 쓸 수 없으면 `data/secrets.json`(Mac에서는 권한 0600)에
  저장하고 경고를 보여 줍니다. 이 경우 묶음 폴더를 다른 사람에게 넘기지 마세요.
- gjc는 업데이트 검사, 모델 프리셋 온라인 갱신, 텔레메트리가 꺼진 상태로 실행됩니다. (방화벽 기능은 아닙니다.)
- 파일 위치: `data/profiles.json`(설정 목록), `data/agent/models.yml`·`config.yml`(gjc 설정, 자동 생성).
  `models.yml`은 실행·전환할 때마다 다시 만들어지므로 직접 고치지 마세요.
  모델을 바꾸려면 `gjc-connect add`로 같은 이름 또는 새 이름으로 등록하세요.
- gjc 안의 `/model`에서도 저장된 모든 설정을 고를 수 있습니다.

### 홈 폴더에 생기는 파일

| 위치 | 내용 | 비고 |
| --- | --- | --- |
| `~/.gjc/natives/<버전>` | gjc가 내장 네이티브 라이브러리를 푼 캐시 | **gjc 자체 동작이라 위치를 바꿀 수 없습니다.** 지워도 다음 실행 때 다시 만들어집니다 |
| `~/.gjc-connect/logs` | gjc 로그 | gjc가 기록합니다 |

gjc-connect는 `~/.gjc` 아래의 설정·세션을 **읽지도 쓰지도 않습니다.** 위 `natives` 캐시만 gjc가 만듭니다.
설정과 API 키 관련 파일은 모두 묶음 폴더의 `data/` 안에 있습니다.

## 문제 해결

| 증상 | 확인할 항목 |
| --- | --- |
| `Windows와 Mac만 지원합니다` | Linux 등 지원하지 않는 OS입니다. Windows x64 또는 Mac에서 실행하세요 |
| `gjc 실행 파일을 찾을 수 없습니다` | 묶음을 다시 풀어 `bin` 폴더가 `gjc-connect`와 같은 위치에 있는지 확인 |
| `서버에 연결할 수 없습니다` | 사내망·VPN 연결, 서버 주소·포트, 방화벽 확인. 응답이 느리면 `GJC_CONNECT_TIMEOUT_SECS=30`처럼 제한 시간을 늘림 |
| `API 키가 맞지 않습니다` | 키·토큰 재입력. 인증이 없는 서버면 '인증 없음' 선택 |
| `서버에 이 모델이 없습니다` | 서버 `/v1/models`의 모델 이름과 대소문자까지 똑같이 입력. 서버가 `/models`를 제공하지 않으면(404/405) 관리자에게 받은 모델 이름 확인 |
| 모델 목록이 안 뜨고 모델 이름을 직접 입력하라고 함 | `/models`를 제공하지 않는 게이트웨이입니다. 정상 동작이며, 모델 이름을 직접 입력하면 됩니다 |
| 홈 폴더에 `~/.gjc/natives`가 생김 | gjc 자체 동작입니다(위치 변경 불가). gjc-connect는 `~/.gjc`의 설정·세션을 읽거나 쓰지 않습니다. 로그는 `~/.gjc-connect/logs` |
| `서버 응답을 이해할 수 없습니다` | 접속 방식(직접/게이트웨이), 주소 끝의 `/v1`, `http`/`https` 확인 |
| Bash 실행 실패 (Windows) | `tools\PortableGit\bin\bash.exe`가 있는지, 또는 Git for Windows 설치 여부 확인. 다른 위치면 `GJC_BASH_PATH`에 `bash.exe` 경로 지정 |
| 컨텍스트 초과 | 서버 `--max-model-len`을 확인. `gjc-connect test`로 연결하면 서버가 알려 준 값이 자동 반영됨 |
| 도구 호출이 텍스트로만 출력 | vLLM의 tool parser 설정(`--enable-auto-tool-choice`, 모델에 맞는 `--tool-call-parser`) 확인 |
| 설정이 꼬였을 때 | `data/` 폴더를 백업한 뒤 삭제하고 `gjc-connect`를 다시 실행 (키체인의 `gjc-connect` 항목은 `remove`로 정리) |

Git Bash 탐색 순서(Windows): `GJC_BASH_PATH` → 묶음의 `tools\PortableGit` → `%ProgramFiles%\Git` → `%LOCALAPPDATA%\Programs\Git`.

## 묶음 만들기 (인터넷이 되는 macOS 빌드 PC)

```bash
cd gjc-connect
./scripts/build-bundle.sh                      # gjc 0.19.3, 가능한 모든 대상
./scripts/build-bundle.sh --version 0.19.3 --targets macos-arm64,windows-x64 \
    --portable-git /path/to/PortableGit        # PortableGit 폴더(또는 PortableGit-*.7z.exe)를 Windows 묶음에 포함
```

결과물은 `dist/gjc-connect-<대상>-<버전>.zip|tar.gz`와 `dist/SHA256SUMS`입니다.
gjc 바이너리는 GitHub 릴리스의 `gajae-release-binaries.sha256`과 대조하며, 일치하지 않으면 중단합니다.

필요한 Rust 타깃 (Rust 1.80 이상):

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
# Windows 묶음은 macOS에서 교차 빌드가 필요합니다. 아래 중 하나를 준비하세요.
cargo install cargo-xwin && rustup target add x86_64-pc-windows-msvc
# 또는 cargo-zigbuild + zig + rustup target add x86_64-pc-windows-gnu
# 또는 mingw-w64 + rustup target add x86_64-pc-windows-gnu
```

교차 빌드 도구가 없으면 해당 대상은 "건너뜀"으로 표시되고 나머지 대상만 만들어집니다
(`--strict`를 주면 건너뛴 대상이 있을 때 실패로 끝납니다). Windows 묶음은 Windows PC에서 직접
`cargo build --release`로 만든 `gjc-connect.exe`를 같은 구조로 넣어도 됩니다.

## 개발

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

개발 중에는 `GJC_CONNECT_HOME=<묶음 구조의 폴더>`로 묶음 폴더를 지정할 수 있습니다.
