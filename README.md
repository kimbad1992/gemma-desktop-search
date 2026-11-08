# Gemma Search

> Local multimodal semantic file search for Windows — Rust + egui GUI on top of Google's **EmbeddingGemma 2** running in **llama.cpp**. Search documents, code, PDFs, photos, video and audio by meaning, fully offline.

EmbeddingGemma 2와 llama.cpp로 만든 Windows용 **로컬 의미 검색기**(Rust, egui)입니다.
문서·코드·PDF·DOCX·사진·영상·오디오를 하나의 768차원 공간에 임베딩해서, 자연어나 이미지로 검색합니다.
모든 처리는 PC 안에서 이뤄지고, 외부로 전송되는 데이터는 없습니다.

| 검색 | 인덱싱 진행 상태 | 서버 관리·자가진단 |
|---|---|---|
| ![검색](docs/screenshots/search.png) | ![인덱스](docs/screenshots/index.png) | ![서버](docs/screenshots/server.png) |

## 준비물

| 항목 | 받는 곳 | 비고 |
|---|---|---|
| llama.cpp (Windows 빌드) | [ggml-org/llama.cpp releases](https://github.com/ggml-org/llama.cpp/releases) | `gemma-embedding2`를 지원하는 b11457 이상. NVIDIA GPU라면 `llama-bXXXX-bin-win-cuda-*-x64.zip`과 `cudart-llama-bin-win-cuda-*-x64.zip`을 같은 폴더에 풉니다 |
| EmbeddingGemma 2 GGUF | [ggml-org/embeddinggemma-2-GGUF](https://huggingface.co/ggml-org/embeddinggemma-2-GGUF) | `embeddinggemma-2-BF16.gguf`(또는 Q8_0)와, 이미지·오디오용 `mmproj-embeddinggemma-2-BF16.gguf`. fp16은 쓰지 마세요(모델 카드 경고) |
| ffmpeg / ffprobe | `winget install Gyan.FFmpeg` 등 | 선택 사항. 영상·오디오·HEIC 인덱싱에 필요 |

기본 경로는 다음 배치를 가정합니다. 다른 곳에 두었다면 설정 탭에서 바꾸면 됩니다.
- `F:\Tools\llama.cpp-embed\` — llama.cpp
- `F:\Tools\models\embeddinggemma-2\` — 모델

## 실행

[Releases](https://github.com/kimbad1992/gemma-desktop-search/releases)에서 zip을 받아 원하는 폴더에 풀고 `gemma-search.exe`를 실행합니다. 직접 빌드하는 방법은 아래 [빌드](#빌드)에 있습니다.

- exe는 코드 서명이 되어 있지 않습니다. 처음 실행할 때 Windows SmartScreen이 "PC 보호" 경고를 띄우면 **추가 정보 → 실행**을 누르세요.

- 설정·인덱스는 exe 옆의 `data\` 폴더에 저장됩니다 (`config.toml`, `index.db`).
- `cargo run`이나 `target\release\gemma-search.exe`로 실행하면 프로젝트 루트의 `data\`를 씁니다.
- 환경 변수 `GEMMA_SEARCH_DATA`로 데이터 폴더를 바꿀 수 있습니다.

## 처음 사용 순서

1. **설정** 탭에서 세 경로를 지정합니다. 옆에 ✔/✖ 표시로 경로가 맞는지 바로 확인됩니다.
   - llama.cpp: 폴더나 `llama-server.exe`
   - 모델 `.gguf`
   - mmproj `.gguf`: 이미지·오디오 임베딩에 필요합니다. 비워 두면 텍스트 전용으로 동작합니다.
2. **인덱싱할 폴더**를 추가하고 **저장**합니다.
3. **서버** 탭에서 **▶ 서버 시작**을 누릅니다.
   - 서버가 준비되면 텍스트·이미지·오디오 자가진단이 자동으로 실행됩니다.
4. **인덱스** 탭에서 **▶ 인덱싱 시작**을 누릅니다.
5. **검색** 탭에서 검색합니다.
   - 자연어 질의(한국어·영어)를 입력할 수 있습니다.
   - 이미지를 창에 끌어다 놓으면 비슷한 이미지·영상을 찾습니다.

## 상태 표시

| 색 | 서버 | 인덱싱 |
|---|---|---|
| 🟢 초록(깜빡임) | 준비됨 | 진행 중 |
| 🟡 노랑 | 시작 중 / 모델 로딩 | 일시정지 / 중지하는 중 |
| ⚪ 회색 | 중지됨 | 대기 / 중지됨 |
| 🔵 파랑 | – | 완료 |
| 🔴 빨강 | 오류 (원인 표시) | 오류 |

**인덱스 탭**
- 진행 막대: 폴더 탐색 중에는 "발견 N개"가 표시되고, 탐색이 끝나면 %로 바뀝니다.
- 카운터: 발견 · 처리 완료 · 변경 없음 · 이름만 · 오류 · 벡터 수
- 종류별 표, 속도, 남은 시간, 현재 처리 중인 파일, 오류 목록

**항상 보이는 표시**
- 하단 상태바: 서버 상태, 인덱스 크기, 인덱싱 %, 마지막 검색 결과 수
- 창 제목: 진행률을 보여줍니다 (예: `[인덱싱 45%] Gemma Search`).

## 검색 결과

모달리티마다 점수 분포가 다릅니다. 그래서 결과를 하나로 섞지 않고 **섹션별로 순위**를 매깁니다.

- **문서 내용**: 텍스트 청크. 일치한 부분의 스니펫이 표시됩니다.
- **이미지**: 썸네일 그리드
- **영상**: 일치한 장면의 시각(예: `00:05 장면`). 오디오 트랙 구간도 포함됩니다.
- **오디오**: 일치한 구간 시작 시각
- **파일 이름**: 모든 파일의 이름과 경로

**최소 일치도**
- 검색 탭의 슬라이더로 이 점수 미만인 결과를 숨깁니다. 다시 검색하지 않고 바로 적용되고, 값은 자동으로 저장됩니다.
- 대체로 0.7 이상이면 잘 맞는 결과입니다.
- 필터는 섹션마다 가져온 최대 결과 수(설정의 "섹션별 최대 결과 수") 안에서 적용됩니다.

**조작**
- 결과를 더블클릭하면 파일이 열립니다.
- "폴더에서 보기"를 누르면 탐색기에서 해당 파일이 선택됩니다.
- 이미지 카드에서는 우클릭 메뉴를 쓸 수 있습니다.

## 동작 방식

- **서버**
  - `llama-server --embeddings --pooling mean`을 콘솔 창 없이 직접 실행·중지합니다.
  - 서버는 Windows Job Object에 묶여 있어서, 앱이 비정상 종료돼도 함께 종료됩니다.
- **접두사**: 모델 카드에 정의된 접두사를 텍스트에만 붙입니다.
  - 질의: `task: search result | query: …`
  - 문서: `title: 파일명 | text: …`
- **영상**: ffmpeg로 프레임을 고르게 최대 12장(설정 가능) 뽑아 이미지로 임베딩합니다.
- **오디오**: 16kHz mono로 변환하고 30초 구간으로 나눠 임베딩합니다. 이 기능은 실험적입니다.
- **증분 인덱싱**
  - 크기와 수정 시각이 같은 파일은 건너뜁니다. 지워진 파일은 인덱스에서 제거합니다.
  - 오류였던 파일은 다음 실행에서 다시 시도합니다.
- **이름만 인덱싱되는 경우**: 0바이트 빈 파일과 암호화된 PDF는 내용을 읽을 수 없습니다. 이런 파일은 오류로 세지 않고 "이름만"으로 분류합니다.
- **차원**
  - DB에는 항상 768차원 원본을 저장합니다.
  - 검색 차원(768/512/256)은 설정에서 바꿀 수 있고, 바꿔도 재인덱싱이 필요 없습니다.
- **재구축이 필요한 경우**: 모델이나 mmproj 파일을 바꾸면 인덱스와 모델이 맞지 않게 됩니다. 이때는 **전체 재구축**을 실행해야 합니다.

## 명령줄 옵션

```
gemma-search.exe --start-server          # 시작하면서 서버 실행
gemma-search.exe --index                 # 서버 준비 후 변경분 인덱싱 자동 시작
gemma-search.exe --query "회의록 예산"     # 서버 준비(및 인덱싱 완료) 후 검색
gemma-search.exe --tab index|server|settings|search
gemma-search.exe --headless-test <폴더> [질의...]   # GUI 없이 전체 흐름 점검 (개발용, 임시 DB 사용)
gemma-search.exe --bench <텍스트파일>              # 실행 중인 서버의 임베딩 처리량 측정
```

기본 서버 옵션은 `-c 16384 -np 8 -b 8192 -ub 8192`입니다.
RTX 4090에서 측정한 처리량은 다음과 같고, 그중 가장 빨랐던 값을 기본으로 정했습니다.

| 설정 | 처리량 |
|---|---|
| `-np 4 -ub 2048` | 약 56 벡터/초 |
| `-np 8 -ub 8192` (기본값) | 약 91 벡터/초 |
| `-np 16` | 약 73 벡터/초 (GPU 포화) |

예: 로그인할 때 `--index`로 실행하면 인덱스를 늘 최신으로 유지할 수 있습니다.

## 지원 형식

| 종류 | 확장자 | 처리 |
|---|---|---|
| 텍스트·코드 | txt, md, csv, json, py, rs, js, ts, c/cpp, … | UTF-8/UTF-16/CP949 자동 판별 후 청크 분할 |
| 문서 | pdf, docx | 텍스트 추출 (스캔 PDF는 이름만) |
| 이미지 | jpg, png, webp, bmp, gif, tiff, heic/avif(ffmpeg) | 이미지 임베딩 + 썸네일 |
| 영상 | mp4, mkv, mov, avi, webm, … | 프레임 + 오디오 트랙 |
| 오디오 | mp3, wav, flac, m4a, ogg, … | 30초 구간 |
| 기타 | 그 밖의 모든 파일 | 파일 이름만 |

현재 지원하지 않는 것: HWP/HWPX, PPTX/XLSX, 스캔 문서 OCR.

## 빌드

```
cargo build --release
cargo test
```

- 필요한 것: Rust(stable, MSVC), Visual Studio C++ 빌드 도구
- 테스트 샘플 만들기: `powershell -ExecutionPolicy Bypass -File testdata\make-testdata.ps1`

## 라이선스

[MIT](LICENSE)

EmbeddingGemma 2 모델 가중치는 이 저장소에 포함되지 않으며, 모델 자체의 라이선스를 따릅니다.
