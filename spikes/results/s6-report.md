# Spike S6 结果：Postman 官方 schema 校验真实导出文件

## 结论

CONDITIONAL PASS：一律按 v2.1.0 严格校验只有 20/23；加上「版本探测 + 版本匹配 schema」后 23/23 全通过。严格校验可行，但前提是先做版本升级。

## 关键数字

| 项目 | 结果 | 验收标准 |
| --- | --- | --- |
| 语料文件数 | 23 个（最大 2.76 MB） | ≥ 20 |
| 策略A：一律按 v2.1.0 严格校验 | 20/23（87.0%） | 20/20 |
| 策略B：按版本匹配 schema 校验 | 23/23（100.0%） | — |
| draft-04 / draft-07 判定不一致 | 0 个 | — |
| 校验耗时 | 单文件最慢 1.37ms，全库合计 7.0ms | — |
| 错误信息不可直接展示占比 | 100% | 越低越好 |

## 官方 schema 加载情况

| 文件 | 字节 | 声明的 draft | 编译 |
| --- | --- | --- | --- |
| `v2.1.0-draft-07.json` | 52960 | `http://json-schema.org/draft-07/schema#` | OK |
| `v2.1.0-draft-04.json` | 55081 | `http://json-schema.org/draft-04/schema#` | OK |
| `v2.0.0-draft-07.json` | 51767 | `http://json-schema.org/draft-07/schema#` | OK |

## 发现

1. 同一个 v2.1.0 版本的 draft-04 与 draft-07 schema 文件内容不同（52960 vs 55081 字节），但对本语料库的判定结果完全一致（不一致 0 个）。说明差异在 schema 写法层面，不影响判定。结论：只需维护 draft-07 一份本地副本。

2. 严格按 v2.1.0 校验：20/23 通过（87.0%）。失败 3 个，全部是 v2.0.0 格式的文件。改用「按探测版本匹配 schema」后通过率升到 100.0%（救回 3 个）。

3. 失败原因不是「schema 太严」，而是「拿 v2.1 的 schema 去校验 v2.0 的文件」。典型差异：v2.0 的 auth 用对象（`{"oauth2":{...}}`），v2.1 改成了 `{key,value,type}` 数组。这说明 03 文档 §8.3 里的 `upgrade_v20.rs` **不是可选优化，是必需的前置步骤**。

4. 错误信息可用性问题：100% 的校验错误来自 oneOf/anyOf 关键字，这类错误只报「不匹配任何分支」并把整棵子树 dump 出来，路径停在容器层（如 `/item/0`），既定位不到具体字段，也无法直接展示给用户。03 文档 §8.3 第 2 条承诺的「报错精确到 JSON Pointer 级字段路径」按当前实现做不到。

5. 校验性能不是问题：最大文件 2.8MB，单文件最慢校验 1.37ms，全库 23 个文件合计 7.0ms。schema 校验完全可以放在导入预览的同步路径上，不需要异步化。

6. 版本探测：全部语料都能从 `info.schema` 的 URL 读到版本，靠内容不靠文件名是可行的（§8.3 第 1 条成立）。注意 v1 格式没有 `info` 字段，必须走结构推断分支 —— 语料库里的 v1 文件在采集阶段就因为缺 `info` 被过滤掉了，这个分支目前**没有被覆盖**，需要单独补测试。

## 建议

1. §8.3 第 2 条需要补充实现细节：oneOf 失败时不能直接用校验器的错误，必须自己按 `item` / `auth` 的判别字段选定预期分支，再对该分支单独校验，才能拿到可用的字段级报错。

2. 导入器采用「探测版本 → 升级到 v2.1 → 严格校验」三段式。schema 校验保留为硬门槛（能挡住真正损坏的文件），但**必须在升级之后**执行。

3. 03 文档 §8.3 第 4 条（校验失败降级为警告）仍然要保留，但定位从「默认策略」调整为「兜底策略」：升级后仍校验失败的文件，降级为警告继续解析并写入迁移报告，不阻断导入。

## 错误类型分布

| 次数 | 类型 |
| --- | --- |
| 10 | 联合类型不匹配（oneOf/anyOf，错误信息不可用） |

## 逐文件明细

| 严格 | 匹配 | 文件 | 字节 | 探测版本 | 探测方式 | 请求数 | 严格err | 校验ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| PASS | PASS | `azdo-Azure DevOps v5.0.postman_collection.json` | 395728 | v2.1.0 | schema-url | 135 | 0 | 0.16 |
| **FAIL** | PASS | `commercetools-checkout_collection.json` | 122439 | v2.0.0 | schema-url | 28 | 3 | 1.22 |
| **FAIL** | PASS | `commercetools-connect_collection.json` | 110820 | v2.0.0 | schema-url | 25 | 4 | 1.00 |
| **FAIL** | PASS | `commercetools-import_collection.json` | 158557 | v2.0.0 | schema-url | 30 | 3 | 1.37 |
| PASS | PASS | `cosmosdb-CosmosDB.postman_collection.json` | 26594 | v2.1.0 | schema-url | 16 | 0 | 0.02 |
| PASS | PASS | `newman-failed-request.json` | 407 | v2.0.0 | schema-url | 1 | 0 | 0.00 |
| PASS | PASS | `newman-nested-requests-fail-report-test.json` | 1496 | v2.1.0 | schema-url | 1 | 0 | 0.00 |
| PASS | PASS | `newman-nested-requests-report-test.json` | 1478 | v2.1.0 | schema-url | 1 | 0 | 0.00 |
| PASS | PASS | `newman-newman-report-test.json` | 1159 | v2.0.0 | schema-url | 1 | 0 | 0.00 |
| PASS | PASS | `newman-response-bodies.json` | 1706 | v2.0.0 | schema-url | 3 | 0 | 0.00 |
| PASS | PASS | `newman-single-file-inside.json` | 1514 | v2.0.0 | schema-url | 2 | 0 | 0.01 |
| PASS | PASS | `newman-single-file-outside.json` | 1529 | v2.0.0 | schema-url | 2 | 0 | 0.00 |
| PASS | PASS | `newman-single-get-request.json` | 540 | v2.0.0 | schema-url | 1 | 0 | 0.00 |
| PASS | PASS | `newman-single-request-failing.json` | 693 | v2.0.0 | schema-url | 1 | 0 | 0.00 |
| PASS | PASS | `newman-ssl-client-cert-list.json` | 1752 | v2.1.0 | schema-url | 2 | 0 | 0.00 |
| PASS | PASS | `newman-ssl-client-cert.json` | 990 | v2.1.0 | schema-url | 1 | 0 | 0.00 |
| PASS | PASS | `newman-undefined-test-checks.json` | 937 | v2.0.0 | schema-url | 1 | 0 | 0.00 |
| PASS | PASS | `newmanex-sample-collection.json` | 1670 | v2.0.0 | schema-url | 3 | 0 | 0.00 |
| PASS | PASS | `real-inardua-integration.json` | 19225 | v2.1.0 | schema-url | 15 | 0 | 0.03 |
| PASS | PASS | `real-zatca.json` | 86344 | v2.1.0 | schema-url | 18 | 0 | 0.03 |
| PASS | PASS | `sabre-Booking-Management_Booking Management API v2025.02.postman_collection.json` | 2690959 | v2.1.0 | schema-url | 916 | 0 | 1.05 |
| PASS | PASS | `sabre-Booking-Management_Booking Management API v2025.05.postman_collection.json` | 2795638 | v2.1.0 | schema-url | 951 | 0 | 1.04 |
| PASS | PASS | `sabre-Booking-Management_Booking Management API v2025.08.postman_collection.json` | 2892928 | v2.1.0 | schema-url | 981 | 0 | 1.06 |

## 失败样本的错误明细

### `commercetools-checkout_collection.json`（探测版本 v2.0.0）

- **instance** `/auth`
  **schema** `/properties/auth/oneOf`
  **分类** 联合类型不匹配（oneOf/anyOf，错误信息不可用）（可直接展示：否）
  ```
  {"oauth2":{"accessToken":"{{ctp_access_token}}","addTokenTo":"header","tokenType":"Bearer"},"type":"oauth2"} is not valid under any of the schemas listed in the 'oneOf' keyword
  ```
- **instance** `/item/0`
  **schema** `/properties/item/items/oneOf`
  **分类** 联合类型不匹配（oneOf/anyOf，错误信息不可用）（可直接展示：否）
  ```
  {"description":"Authorization","item":[{"event":[{"listen":"test","script":{"exec":["tests[\"Status code is 200\"] = responseCode.code === 200;","var data = JSON.parse(responseBody);","if(data.access_token){","    pm.environment.set(\"ctp_a…
  ```
- **instance** `/item/1`
  **schema** `/properties/item/items/oneOf`
  **分类** 联合类型不匹配（oneOf/anyOf，错误信息不可用）（可直接展示：否）
  ```
  {"description":"null","item":[{"description":"null","item":[{"event":[{"listen":"test","script":{"exec":["tests[\"Status code \" + responseCode.code] = responseCode.code === 200 || responseCode.code === 201;","var data = JSON.parse(response…
  ```

### `commercetools-connect_collection.json`（探测版本 v2.0.0）

- **instance** `/auth`
  **schema** `/properties/auth/oneOf`
  **分类** 联合类型不匹配（oneOf/anyOf，错误信息不可用）（可直接展示：否）
  ```
  {"oauth2":{"accessToken":"{{ctp_access_token}}","addTokenTo":"header","tokenType":"Bearer"},"type":"oauth2"} is not valid under any of the schemas listed in the 'oneOf' keyword
  ```
- **instance** `/item/0`
  **schema** `/properties/item/items/oneOf`
  **分类** 联合类型不匹配（oneOf/anyOf，错误信息不可用）（可直接展示：否）
  ```
  {"description":"Authorization","item":[{"event":[{"listen":"test","script":{"exec":["tests[\"Status code is 200\"] = responseCode.code === 200;","var data = JSON.parse(responseBody);","if(data.access_token){","    pm.environment.set(\"ctp_a…
  ```
- **instance** `/item/1`
  **schema** `/properties/item/items/oneOf`
  **分类** 联合类型不匹配（oneOf/anyOf，错误信息不可用）（可直接展示：否）
  ```
  {"description":"The Project endpoint is used to retrieve certain information from a project.","item":[{"description":"null","item":[{"event":[{"listen":"test","script":{"exec":["tests[\"Status code \" + responseCode.code] = responseCode.cod…
  ```
- **instance** `/item/2`
  **schema** `/properties/item/items/oneOf`
  **分类** 联合类型不匹配（oneOf/anyOf，错误信息不可用）（可直接展示：否）
  ```
  {"description":"null","item":[{"event":[{"listen":"test","script":{"exec":["tests[\"Status code \" + responseCode.code] = responseCode.code === 200 || responseCode.code === 201;","var data = JSON.parse(responseBody);","if(data.results && da…
  ```

### `commercetools-import_collection.json`（探测版本 v2.0.0）

- **instance** `/auth`
  **schema** `/properties/auth/oneOf`
  **分类** 联合类型不匹配（oneOf/anyOf，错误信息不可用）（可直接展示：否）
  ```
  {"oauth2":{"accessToken":"{{ctp_access_token}}","addTokenTo":"header","tokenType":"Bearer"},"type":"oauth2"} is not valid under any of the schemas listed in the 'oneOf' keyword
  ```
- **instance** `/item/0`
  **schema** `/properties/item/items/oneOf`
  **分类** 联合类型不匹配（oneOf/anyOf，错误信息不可用）（可直接展示：否）
  ```
  {"description":"Authorization","item":[{"event":[{"listen":"test","script":{"exec":["tests[\"Status code is 200\"] = responseCode.code === 200;","var data = JSON.parse(responseBody);","if(data.access_token){","    pm.environment.set(\"ctp_a…
  ```
- **instance** `/item/1`
  **schema** `/properties/item/items/oneOf`
  **分类** 联合类型不匹配（oneOf/anyOf，错误信息不可用）（可直接展示：否）
  ```
  {"description":"null","item":[{"description":"null","item":[{"event":[{"listen":"test","script":{"exec":["tests[\"Status code \" + responseCode.code] = responseCode.code === 200 || responseCode.code === 201;","var data = JSON.parse(response…
  ```

