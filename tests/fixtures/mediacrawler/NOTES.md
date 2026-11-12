# Fixture notes

All files here are hand-written with invented content. NOT MEASURED: nobody involved ran MediaCrawler or had a real result file, so none of this has been compared with one.

- Shape read from NanmiCoder/MediaCrawler at commit `306ab4ca26c0e44fb87ede0e40e98748752815a6` (2026-11-09), files `store/zhihu/__init__.py（`update_zhihu_content`）和 model/m_zhihu.py（`ZhihuContent`、`ZhihuComment`）` and `tools/async_file_writer.py` (output goes to `data/<platform>/<json|jsonl|csv>/<crawler type>_<contents|comments>_<date>.<ext>`; `json` is a growing array written with `indent=4`, `jsonl` is one object per line and is the repository's default `SAVE_DATA_OPTION` in `config/base_config.py`). Fields used by the plugin: content_id, content_type（answer、article、zvideo）, title, desc, created_time, user_nickname, question_id（`model/m_zhihu.py` 的 `ZhihuContent`）. Only field names were read; no code was copied. Its licence is a non-commercial learning licence without distribution rights, so nothing of it is bundled here.
- `search_contents_2026-11-01.json` and `.jsonl`: the same five posts (plus one zvideo). Two have publish times, one has none (`0`), one repeats the first post's id, one has an id that is not an id.
- `search_comments_2026-11-01.json` and `.jsonl`: comment records (they carry `comment_id`); the plugin refuses them.
- `changed.json`: posts whose ids are all unusable, for the "format changed" answer.
- MediaCrawler masks the nickname (first and last character kept) and stores a hash instead of the user id, so the author name listed is the masked one and there is no author id.
- The `last_modify_ts` field is the time MediaCrawler saved the record; the plugin never reads it.
