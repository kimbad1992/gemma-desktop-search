# 테스트용 샘플 파일 생성 (Windows PowerShell 5.1 호환)
# 사용: powershell -ExecutionPolicy Bypass -File testdata\make-testdata.ps1
$ErrorActionPreference = 'Stop'
$root = Join-Path $PSScriptRoot 'samples'
New-Item -ItemType Directory -Force $root, "$root\문서", "$root\사진", "$root\영상", "$root\소리", "$root\코드" | Out-Null
$utf8 = New-Object Text.UTF8Encoding($false)

[IO.File]::WriteAllText("$root\문서\2024 예산 회의록.txt", @"
2024년 3분기 예산 회의록
참석자: 김부장, 이과장, 박대리
1. 마케팅 예산을 15% 증액하기로 결정했다.
2. 신규 서버 구매 비용 3천만 원은 다음 분기로 연기한다.
3. 출장비 정산 절차를 간소화한다.
"@, $utf8)

# CP949(EUC-KR)로 저장된 레시피
[IO.File]::WriteAllText("$root\문서\김치찌개.txt", @"
김치찌개 끓이는 법
1. 돼지고기 앞다리살을 참기름에 볶는다.
2. 잘 익은 김치를 넣고 함께 볶는다.
3. 물을 붓고 두부와 대파를 넣어 20분간 끓인다.
신 김치를 쓰면 맛이 더 깊어진다.
"@, [Text.Encoding]::GetEncoding(949))

[IO.File]::WriteAllText("$root\문서\account-help.md", @"
# Account help
If you forgot your password, click **Forgot password** on the sign-in page.
We will email you a reset link that expires in 30 minutes.
"@, $utf8)

[IO.File]::WriteAllText("$root\코드\sort.rs", @"
/// Classic quicksort implementation using Lomuto partitioning.
pub fn quicksort<T: PartialOrd>(v: &mut [T]) {
    if v.len() <= 1 { return; }
    let p = partition(v);
    quicksort(&mut v[..p]);
    quicksort(&mut v[p + 1..]);
}
fn partition<T: PartialOrd>(v: &mut [T]) -> usize {
    let last = v.len() - 1;
    let mut i = 0;
    for j in 0..last { if v[j] <= v[last] { v.swap(i, j); i += 1; } }
    v.swap(i, last);
    i
}
"@, $utf8)

# --- 최소 PDF (Helvetica) ---
$content = "BT /F1 18 Tf 72 720 Td (Quarterly solar panel installation report) Tj 0 -28 Td (The photovoltaic array on the factory roof produced 12 MWh in March.) Tj ET"
$objs = @(
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    "<< /Length $($content.Length) >>`nstream`n$content`nendstream",
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"
)
$sb = New-Object Text.StringBuilder
[void]$sb.Append("%PDF-1.4`n")
$offsets = @()
for ($i = 0; $i -lt $objs.Count; $i++) {
    $offsets += $sb.Length
    [void]$sb.Append("$($i + 1) 0 obj`n$($objs[$i])`nendobj`n")
}
$xref = $sb.Length
[void]$sb.Append("xref`n0 $($objs.Count + 1)`n0000000000 65535 f `n")
foreach ($o in $offsets) { [void]$sb.Append(("{0:D10} 00000 n `n" -f $o)) }
[void]$sb.Append("trailer`n<< /Size $($objs.Count + 1) /Root 1 0 R >>`nstartxref`n$xref`n%%EOF`n")
[IO.File]::WriteAllText("$root\문서\solar-report.pdf", $sb.ToString(), [Text.Encoding]::ASCII)

# --- 최소 DOCX ---
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$docx = "$root\문서\여행 계획.docx"
if (Test-Path $docx) { Remove-Item $docx -Force }
$zip = [IO.Compression.ZipFile]::Open($docx, 'Create')
function Add-Entry($zip, $name, $text) {
    $e = $zip.CreateEntry($name)
    $w = New-Object IO.StreamWriter($e.Open(), (New-Object Text.UTF8Encoding($false)))
    $w.Write($text); $w.Close()
}
Add-Entry $zip '[Content_Types].xml' '<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>'
Add-Entry $zip '_rels/.rels' '<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>'
Add-Entry $zip 'word/document.xml' '<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>제주도 가족 여행 계획</w:t></w:r></w:p><w:p><w:r><w:t>첫째 날: 성산일출봉 등반 후 해변에서 일몰 감상</w:t></w:r></w:p><w:p><w:r><w:t>둘째 날: 한라산 국립공원 트레킹, 흑돼지 저녁 식사</w:t></w:r></w:p></w:body></w:document>'
$zip.Dispose()

# --- 이미지 / 영상 / 오디오 (ffmpeg) ---
function FF { & ffmpeg -hide_banner -v error -y @args; if ($LASTEXITCODE -ne 0) { throw "ffmpeg failed: $args" } }
FF -f lavfi -i 'color=c=red:s=640x480' '-frames:v' 1 "$root\사진\IMG_0001.jpg"
FF -f lavfi -i 'color=c=0x1E50DC:s=640x480' '-frames:v' 1 "$root\사진\IMG_0002.jpg"
FF -f lavfi -i 'mandelbrot=s=640x480' '-frames:v' 1 "$root\사진\IMG_0003.png"
FF -f lavfi -i 'testsrc2=s=640x480' '-frames:v' 1 "$root\사진\IMG_0004.jpg"
# 앞 4초 빨강, 뒤 4초 파랑 + 440Hz 소리
FF -f lavfi -i 'color=c=red:s=640x360:d=4' -f lavfi -i 'color=c=0x1E50DC:s=640x360:d=4' -f lavfi -i 'sine=frequency=440:duration=8' -filter_complex '[0:v][1:v]concat=n=2:v=1:a=0[v]' -map '[v]' -map 2:a '-c:v' libx264 -pix_fmt yuv420p '-c:a' aac -shortest "$root\영상\clip_001.mp4"
FF -f lavfi -i 'sine=frequency=880:duration=6' '-c:a' libmp3lame "$root\소리\beep.mp3"
FF -f lavfi -i 'anoisesrc=d=6:c=pink' '-c:a' pcm_s16le "$root\소리\rec_0001.wav"

Get-ChildItem $root -Recurse -File | Select-Object @{n='file';e={$_.FullName.Substring($root.Length + 1)}}, Length | Format-Table -AutoSize | Out-String -Width 200
