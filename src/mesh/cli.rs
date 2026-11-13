//! 창 없이 원격 라이브러리를 시험하는 숨은 명령 (앱 화면과 같은 함수를 쓴다)
//!
//!   gemma-search --mesh-join <참여 코드>        메시 기기로 넣는다 (설정의 mesh_devices)
//!   gemma-search --mesh-add <네트워크 폴더> [<참여 코드>]
//!                                              원격 라이브러리로 붙인다 (설정의 remote_libraries). 모르는 서버면
//!                                              Gemma Search가 있는지 알려 주고, 참여 코드를 주면 참여하고 붙인다
//!   gemma-search --mesh-status                  메시 기기 상태와 원격 라이브러리
//!   gemma-search --mesh-search <질의…>          원격 라이브러리만 검색해 이 기기에서 여는 경로와 함께 보인다
//!
//! 쓰고 있는 설정과 섞이지 않게 `GEMMA_SEARCH_DATA`로 시험용 데이터 폴더를 준다.

use super::client::{self, AddOutcome, Owner, RemoteQuery};
use super::JoinCode;
use crate::config::Config;
use std::time::Duration;

fn load() -> (std::path::PathBuf, Config) {
    let dir = crate::config::data_dir();
    let cfg = Config::load(&dir);
    (dir, cfg)
}

pub fn join(args: &[String]) -> i32 {
    let Some(text) = args.first() else {
        eprintln!("참여 코드를 주세요");
        return 2;
    };
    let code = match JoinCode::decode(text) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let d = match client::join(&code) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let (dir, mut cfg) = load();
    println!("넣음: {} ({}) 주소 {}", d.name, d.id, d.addrs.join(", "));
    match cfg.mesh_devices.iter_mut().find(|x| x.id == d.id) {
        Some(x) => *x = d,
        None => cfg.mesh_devices.push(d),
    }
    if let Err(e) = cfg.save(&dir) {
        eprintln!("설정을 저장하지 못했습니다: {e}");
        return 1;
    }
    0
}

pub fn add(args: &[String]) -> i32 {
    let Some(path) = args.first() else {
        eprintln!("네트워크 폴더를 주세요");
        return 2;
    };
    let (dir, mut cfg) = load();
    let outcome = match args.get(1) {
        Some(text) => match JoinCode::decode(text) {
            Ok(code) => client::join_and_add(&code, path).map(|(device, library, reply)| {
                println!("참여함: {} ({}) 주소 {}", device.name, device.id, device.addrs.join(", "));
                match cfg.mesh_devices.iter_mut().find(|x| x.id == device.id) {
                    Some(x) => *x = device.clone(),
                    None => cfg.mesh_devices.push(device.clone()),
                }
                AddOutcome::Attached { device, library, reply }
            }),
            Err(e) => Err(e),
        },
        None => client::add_network_folder(path, &cfg.mesh_devices),
    };
    match outcome {
        Ok(AddOutcome::NotMesh(why)) => {
            println!("메시 기기의 폴더가 아님 (이 기기가 직접 인덱싱할 폴더): {why}");
            3
        }
        Ok(AddOutcome::Detected { server, hello }) => {
            println!(
                "{server}에서 Gemma Search v{}({}, 기기 ID {})가 돌고 있습니다. 참여 받기: {}. 참여 코드를 함께 주면 붙입니다",
                hello.version,
                hello.name,
                hello.id,
                if hello.join { "예" } else { "아니오(이 기기에서만 열림)" }
            );
            4
        }
        Ok(AddOutcome::NoService { server }) => {
            println!("{server}에서 Gemma Search를 찾지 못했습니다 (포트 {}). 이 기기가 직접 인덱싱할 폴더입니다", client::DEFAULT_PORT);
            3
        }
        Ok(AddOutcome::Attached { device, library, reply }) => {
            println!(
                "붙임: {} · 라이브러리 {} ({}) · 소유 기기 폴더 {} · 새로 만듦 {} · 인덱싱 {} · 표본 {}/{}",
                device.name,
                reply.library.name,
                reply.library.id,
                reply.folder,
                reply.created,
                reply.indexing,
                reply.matched,
                reply.checked
            );
            println!("이 기기에서 여는 폴더: {}", library.local_folder);
            match cfg
                .remote_libraries
                .iter_mut()
                .find(|l| l.device_id == library.device_id && l.owner_folder == library.owner_folder)
            {
                Some(l) => *l = library,
                None => cfg.remote_libraries.push(library),
            }
            if let Err(e) = cfg.save(&dir) {
                eprintln!("설정을 저장하지 못했습니다: {e}");
                return 1;
            }
            0
        }
        Err(e) => {
            eprintln!("붙이지 못함: {e}");
            1
        }
    }
}

pub fn status(_args: &[String]) -> i32 {
    let (_, cfg) = load();
    for d in &cfg.mesh_devices {
        let owner = Owner::new(d, None, Duration::from_secs(8));
        match owner.json("GET", "/api/status", None) {
            Ok((base, v)) => println!(
                "{} {base}: 파일 {} · 인덱싱 {} ({}/{})",
                d.name,
                v["index"]["files"],
                v["indexing"]["state"].as_str().unwrap_or(""),
                v["indexing"]["processed"],
                v["indexing"]["discovered"]
            ),
            Err(e) => println!("{}: {e}", d.name),
        }
        match client::libraries(&owner) {
            Ok(libs) => {
                for l in libs {
                    println!("  라이브러리 {} {} · 파일 {} · 표본 {}개", l.id, l.path, l.files, l.samples.len());
                }
            }
            Err(e) => println!("  라이브러리 목록: {e}"),
        }
    }
    for l in &cfg.remote_libraries {
        println!("원격 라이브러리: {} → {} (켬 {}, 빌려주기 {})", l.local_folder, l.owner_folder, l.enabled, l.lend_while_indexing);
    }
    0
}

pub fn search(args: &[String]) -> i32 {
    let q = args.join(" ");
    if q.trim().is_empty() {
        eprintln!("질의를 주세요");
        return 2;
    }
    let (_, cfg) = load();
    let mut code = 0;
    for (index, d) in cfg.mesh_devices.iter().enumerate() {
        let libraries: Vec<_> = cfg.remote_libraries.iter().filter(|l| l.enabled && l.device_id == d.id).cloned().collect();
        if libraries.is_empty() {
            continue;
        }
        let rq = RemoteQuery { index, device: d.clone(), base: None, libraries };
        let a = client::search(&rq, &q, &[true; 5], "");
        if let Some(e) = &a.error {
            println!("{}: {e}", a.device_name);
            code = 1;
            continue;
        }
        println!("{}: 결과 {}개 (벡터 {}개 비교)", a.device_name, a.hits.len(), a.searched);
        let mut hits = a.hits;
        hits.sort_by(|x, y| y.1.hit.score.total_cmp(&x.1.hit.score));
        for (sec, hv) in hits.iter().take(10) {
            let exists = std::path::Path::new(&hv.hit.path).exists();
            println!(
                "  {:>5.1} {:<8} {} [{}]",
                hv.hit.score,
                sec.label(),
                hv.hit.path,
                if exists { "열 수 있음" } else { "이 기기에서 안 보임" }
            );
        }
    }
    code
}
