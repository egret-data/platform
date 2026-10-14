# Test đồng hồ thật hay chập chờn: sống sót trên một máy đang tải nặng

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-14-flaky-wall-clock-tests-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, 2026-10-14. Số dòng là của `main` tại `8e34db0a`.

## 1. Vấn đề

Nhiều test của `egret-workflow` so thời gian đồng hồ thật đã trôi với một ngưỡng cố định, hoặc cho một lần sleep thật chạy đua với một hạn cố định. Test nào chạy riêng cũng qua. Test nào cũng từng đỏ khi `cargo` của các phiên khác chạy cùng máy (load average 23–29 trên 8 lõi):

- `storage::kv::merge::tests::many_keys_near_the_depth_limit_merge_fast` khẳng định `took < 2 s` (`crates/egret-workflow/src/storage/kv/merge.rs:914-917`). Nó đỏ ở 2,84 s ("1638 keys of 100 segments took 2.839154778s"), trong khi chạy riêng chỉ khoảng 1,1 s. Các test đo thời gian khác của `merge` đỏ ở 3,5 s và 5,5 s.
- `tests/worker.rs::a_run_whose_worker_stopped_renewing_is_reaped` (`crates/egret-workflow/tests/worker.rs:9397`) đỏ một lần khi máy tải nặng, và chạy riêng thì qua cả 3/3 lần.

Test đỏ vì máy chậm sẽ dạy người ta chạy lại test đỏ cho đến khi xanh. Gói này làm cho các test ấy phụ thuộc vào mã được kiểm, không phụ thuộc vào máy.

## 2. "Đã sửa" nghĩa là gì

Test đã sửa vẫn bắt được lỗi hồi quy mà nó được viết ra để bắt, và không đỏ vì máy chậm. Thứ tự ưu tiên của các cách làm:

- (a) đo công việc, không đo thời gian;
- (b) dùng đồng hồ ảo, nếu mã vốn đã nhận một đồng hồ;
- (c) dùng ngưỡng tương đối so với một mốc được đo trong cùng lần chạy;
- (d) cùng lắm mới nới một ngưỡng tuyệt đối thật rộng.

Mỗi test đã sửa (§3.1–§3.3) phải được cho thấy là đỏ trên một bản cài đặt cố tình làm hỏng, và qua 5/5 lần khi CPU bị vắt kiệt.

## 3. Các test trong gói này

### 3.1 `merge.rs`: ba test về độ phức tạp

Mã, đã kiểm:

- `refuse_conflicts` đi qua một trie, mỗi segment một lần tra `child` (`merge.rs:222-258`).
- `flatten_to_nested` đi mỗi segment một bước trong arena (`merge.rs:344-390`).
- Giới hạn độ sâu được kiểm trong vòng lặp entry của `merge_entries`, trước khi hai hàm kia chạy (`merge.rs:173-178`).

| Test | Ngưỡng hiện nay | Hồi quy nó canh | Cách đo mới |
|---|---|---|---|
| `a_dotted_key_deeper_than_max_depth_is_refused_at_once` (`merge.rs:847`) | `< 250 ms` | việc từ chối xảy ra sau khi đã dựng một thứ gì đó | (a) công việc: một bộ đếm bước đường đi chỉ có khi test, riêng từng thread (mỗi lần tra `child` trong trie một bước, mỗi bước arena một bước), phải bằng **0** sau một lần merge bị từ chối |
| `many_keys_near_the_depth_limit_merge_fast` (`merge.rs:892`) | `< 2 s` | merge bậc hai theo độ sâu của đường đi (C1: băm mọi tiền tố thành một vector) | (a) đúng 2 bước mỗi segment (ngưỡng: gấp 3 lần số ấy), chống một lượt đi thừa, **và** (c), chống bước đắt hơn của C1, hai patch sâu tốn tối đa 1,5 lần một mốc: cùng số segment nhưng dưới dạng key một segment, đo ngay bên cạnh |
| `a_large_merge_is_linear` (`merge.rs:943`) | `< 5 s` | merge bậc hai theo bề rộng map hay độ dài list | (c) ba lần merge ở cỡ `n` tốn tối đa 30 lần chính ba lần merge ấy ở cỡ `n / 10`. Tuyến tính thì tỉ số khoảng 10; bậc hai thì khoảng 100 |

Bộ đếm có `#[cfg(test)]`. Ở các bản build khác, nó là một hàm inline rỗng.

Ngưỡng tương đối được đo bằng helper `least_times`:

- Nó lấy giá trị nhỏ nhất của 3 lần chạy mốc và giá trị nhỏ nhất của 2 lần chạy ca lớn.
- Nó chạy tối đa 5 vòng và dừng ở vòng đầu tiên lọt ngưỡng. Nó cũng dừng sớm khi ca lớn vượt một tỉ số bỏ cuộc (60 với `a_large_merge_is_linear`), nên một bản merge hồi quy đỏ sau một vòng chậm, không phải năm vòng.
- Tải làm giãn cả hai số đo thì triệt tiêu nhau. Một đợt tải dồn trong một vòng thì vòng sau sống qua được. Chỉ một chi phí tăng nhanh hơn mốc mới đỏ ở mọi vòng.

Số đo ngày 2026-10-14: các patch sâu tốn 0,5–0,7 lần mốc, dù máy tải hay không. Khi đặt lại lỗi C1 (băm mọi tiền tố thành một vector, ở cả bước kiểm lẫn bước lồng), chúng tốn 2,1–2,3 lần; đặt lại một nửa lỗi thì 1,6–1,8 lần. Vì độ sâu bị chặn ở `MAX_DEPTH`, bậc hai theo độ sâu chỉ tốn một hệ số có giới hạn, nên khoảng phân biệt này hẹp. Ngưỡng tuyệt đối 2 s có khoảng còn hẹp hơn: 1,1 s khi chạy riêng và 2,8 s khi máy tải. `a_large_merge_is_linear` đo được 10–15 lần khi tuyến tính, và 108 lần khi `deep_merge` dò tuần tự.

Hai cách đo canh hai kiểu hồi quy khác nhau. Số bước bắt được một **lượt đi thừa**: mutation `trie-rewalk` tốn 16,5 triệu bước so với ngưỡng khoảng 1 triệu. Nó **không thấy được C1**, vì C1 giữ nguyên số bước và làm mỗi bước đắt hơn: các mutation `c1` và `prefix-vectors` tốn đúng 655 200 bước như bản merge tuyến tính. Thời gian tương đối là cái canh **duy nhất** chống C1. Khoảng dư của nó so với một nửa lỗi C1 chỉ khoảng 1,1 lần, nên nửa lỗi ấy thỉnh thoảng có thể lọt: một hồi quy bị bỏ sót, không bao giờ là một lần đỏ trên mã đúng.

### 3.2 `a_run_whose_worker_stopped_renewing_is_reaped`

Mã, đã kiểm:

- `reap` thu hồi lease cũ hơn `timeout` theo đồng hồ của database: `claimed_at < now() - make_interval(secs => $1)` (`worker/reaper.rs:39-53`).
- Worker gia hạn sau mỗi `working_timeout / 3` (`worker/claim.rs:1147-1149`).
- Worker tự rào (self-fence) khi suốt `working_timeout` không lần gia hạn nào thành công (`worker/claim.rs:1616-1618`, `under_lease_with`).
- Một lần gia hạn còn đang bay khi future của worker bị drop vẫn có thể commit trên server (`claim.rs:1493-1495`).

Test hiện nay dùng lease 900 ms và các lần sleep cố định:

- Nó sleep 1,5 s, rồi chờ `reap(900 ms) == 0`. Một lần gia hạn trễ hơn khoảng 600 ms làm run bị thu hồi được, hoặc làm worker tự rào.
- Sau khi abort, nó đọc lease ngay, sleep 1,2 s, rồi chờ lease vẫn như cũ. Một lần gia hạn còn đang bay lúc abort có thể đáp xuống giữa hai lần đọc.

Đồng hồ ảo (b) không dùng được ở đây, vì lease và reaper đều đọc `now()` của Postgres. Test mới chờ theo điều kiện:

- **Lượt còn sống.** Cứ 50 ms đọc lease một lần, cho đến khi có một lần gia hạn đáp xuống sau thời điểm claim hơn `working_timeout`, rồi reap ngay: không thu hồi run nào. Hạn chờ là 60 s, nên một worker không bao giờ gia hạn sẽ đỏ kèm thông báo. `working_timeout` thành 2 s, nên cứ 667 ms gia hạn một lần và khoảng dư là 1,33 s. Phần phụ thuộc thời gian còn lại ấy không bỏ được: một worker còn sống mà không gia hạn được trong `working_timeout` thì bị thu hồi là đúng.
- **Lượt đã chết.** Abort worker, rồi poll `pg_stat_activity` cho đến khi không backend nào khác của database của test còn `active` (hạn báo lỗi 60 s), để lần gia hạn còn đang bay đã đáp xuống. Đọc lease, rồi chờ đến khi **đồng hồ database** vượt nó một `working_timeout` (`until_db_clock_passes`). Lease phải không đổi, và reap thu hồi một run.

Test vẫn bắt được:

- worker không bao giờ gia hạn (lượt còn sống hết hạn chờ);
- reaper thu hồi mọi thứ (lần reap lúc còn sống trả về 1);
- reaper không thu hồi gì (lần reap lúc đã chết trả về 0);
- worker vẫn gia hạn sau khi bị abort (lease nhích đi).

### 3.3 Thêm ba test từ đợt rà soát (HIGH)

| Test | Chạy đua hiện nay | Cách đo mới | Vẫn bắt được |
|---|---|---|---|
| `tests/runtime.rs:1398` `a_save_that_meets_a_claimed_trigger_waits_and_keeps_the_advance` | Việc giữ là một `pg_sleep(1)` trong trigger `BEFORE UPDATE`. Hai lần poll `pg_stat_activity` và một lần save được spawn phải lọt trong một giây ấy. | **Không đo thời gian nào cả.** Trigger chờ `pg_advisory_xact_lock_shared(1386)`, trong khi test giữ `pg_advisory_lock(1386)` trên một connection riêng. Lượt pass được thấy đang chờ (`wait_event = 'advisory'`), rồi lần save được thấy đang chờ (`transactionid` / `tuple`). Chỉ khi đó test mới thả lượt pass. | Một lần save tua ngược bước tiến: nó đọc ngày mà không giữ khoá dòng rồi ghi lại ngày ấy. |
| `tests/task_logs.rs:821` `lines_reach_postgres_in_order_within_a_flush` | `count == 0` phải được đọc trước khi timer flush 300 ms nổ. | (a) Writer chạy trên inserter `Rigged` có đếm: đúng **một** lần gọi lưu ba dòng. Thêm một cận **dưới**, mà tải chỉ làm nó đúng hơn: lần gọi ấy bắt đầu sau lần ghi đầu tiên ít nhất `flush_every`. | Flush mỗi dòng (3 lần gọi); flush không chờ timer (nó bắt đầu khoảng 2 ms sau lần ghi). |
| `tests/task_logs.rs:973` `an_execution_deleted_batch_is_rerun_once_not_retried` | `< 400 ms` qua các lượt đi về Postgres, rồi `sleep(500)` trước khi kiểm `calls == 4`. | (a) "Ngay lập tức" được đo thành "không task nào khác chạy giữa lỗi 23503 và lần chạy lại". Mỗi lần gọi `Rigged` spawn một task dò, task này bật một cờ. Trên runtime một luồng của test, task dò chỉ chạy được khi writer nhường, và một lần `sleep` backoff thì có nhường. Lần gọi thứ 4 được chờ như một điều kiện. "Bị bỏ" được cho thấy bằng việc batch kế tiếp được lưu bởi lần gọi thứ 5, và các dòng bị bỏ không bao giờ được lưu. | Một lần backoff trước khi chạy lại; lỗi 23503 thứ hai bị thử lại như một lỗi thường. |

Task dò trong test thứ ba cần runtime một luồng, nên cả hai test `task_logs` đặt rõ `#[tokio::test(flavor = "current_thread")]`. Trên runtime nhiều luồng, task dò có thể chạy song song với writer và test sẽ đỏ trên mã đúng (đỏ giả); nó sẽ không lặng lẽ yếu đi.

Ba test này được xếp HIGH theo phân tích. Bản trên `main` qua 5/5 lần dưới cùng mức tải, nên chưa tái hiện được lần đỏ nào.

## 4. Không sửa trong gói này

Các test nhạy thời gian sau được đánh giá MEDIUM-HIGH. Chúng được để lại như việc tiếp theo đã biết.

- `tests/runtime.rs:2025` `a_run_longer_than_working_timeout_is_not_reaped_while_its_worker_lives`: `working_timeout` 2 s, gia hạn sau mỗi 667 ms, trong khi reaper nhịp mỗi 100 ms. Một lần gia hạn kẹt quá khoảng 1,3 s làm run bị thu hồi và chạy hai lần.
- `src/worker/claim.rs:4101` `succeeding_renewals_keep_the_run_past_the_deadline`: chạy trên đồng hồ thật với `EVERY` 60 ms và `FENCE` 200 ms. Một runtime bị bỏ đói sẽ tự rào. Đồng hồ dừng (paused) dùng được ở đây, vì `under_lease_with` vốn đã nhận một hàm gia hạn tiêm vào và dùng đồng hồ tokio.
- `tests/prefetch_hold.rs:1752` `the_lease_is_renewed_while_a_long_fetch_downloads`: lease 1,5 s và `sleep(3s)`, rồi `reap == 0`. Đây là đúng mẫu mà §3.2 đã thay; có thể dùng lại `until_db_clock_passes`.
- `tests/kv_store.rs:1408` `a_large_decode_runs_off_the_async_runtime_and_a_ticker_keeps_ticking`: một ticker 2 ms phải nhịp mỗi 10 ms của lần decode. Khi tải cao, riêng độ trễ đánh thức của hệ điều hành đã vượt mức ấy.
- `src/storage/nsfiles/stage.rs:1148` `matching_runs_off_the_async_runtime_and_a_ticker_keeps_ticking`: cùng mức sàn ticker ấy, thêm một phép kiểm trên pool 1 connection.
- `tests/container_docker_live.rs` 795, 986 và 1853 (test live, phải bật mới chạy): mỗi test drop `run()` sau 1,5–2,5 s rồi khẳng định container đang chạy, nên một lần `docker run` chậm thì chưa kịp khởi động nó.

Được đánh giá MEDIUM, cũng để lại như việc tiếp theo:

- `tests/flow_trigger_fire.rs:1476` `a_pass_stops_at_its_time_cap_and_the_next_pass_goes_on`: trần 1 s của một lượt pass so với `pg_sleep(0.6)` mỗi lần fire.
- `tests/runtime.rs:567` `the_worker_ticks_on_a_fixed_interval_rather_than_draining_to_zero`: `sleep(600)`, rồi `ran == 1`.
- `tests/worker.rs:7000` `a_max_duration_chain_stops_when_the_original_clock_runs_out`: lần thử đầu tiên phải xong trong 1 s của ngân sách PT2S.
- `tests/worker.rs:211` `two_concurrent_claims_partition_the_runnable_set`: `timeout(500ms, claim)`.
- `tests/kv_sweep.rs:379` `the_spawned_sweeper_sweeps_after_its_interval_and_stops_on_cancel`: "chưa quét" ở 500 ms so với chu kỳ 1 s.
- `tests/nsfiles_api.rs:1894` `an_export_whose_client_goes_away_stops_opening_files`: ảnh chụp lan truyền abort sau 300 ms.
- `src/worker/commands.rs:4387` `a_dropped_run_future_leaves_no_staging_directory_behind`: hai lần spawn tiến trình trong 1,5 s.
- `src/worker/k8s/runner.rs:2416` `a_slow_but_progressing_upload_is_not_timed_out_and_a_stalled_one_is`: một `select!` không thiên vị với khoảng dư 200 ms.
- `tests/task_logs.rs:1853` `a_refused_line_costs_only_itself_not_its_batch`: barrier `< 2 s` qua các lượt đi về Postgres.

**Về phép kiểm tương đối ở §3.1.** Trong `many_keys_near_the_depth_limit_merge_fast`, khoảng phân biệt của nó hẹp: ngưỡng 1,5, tuyến tính 0,53–0,69, một nửa lỗi C1 là 1,64. Dù vậy nó là cái canh duy nhất chống bước đắt hơn của C1; bộ đếm bước chỉ canh một lượt đi thừa. Khoảng dư hẹp có thể dẫn tới bỏ sót một hồi quy, không bao giờ là một lần đỏ trên mã đúng.

## 5. Bằng chứng cần có

Với mỗi test đã sửa, báo cáo ghi lại:

- mutation và output đỏ của nó;
- một lần qua bình thường;
- 5 lần qua khi CPU bão hoà: 24 tiến trình `yes > /dev/null` trên 8 lõi, do chính gói này khởi động và tắt.
