<!--
Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
-->

# 火山引擎 TOS Bucket API 参考文档

> 数据来源：火山引擎对象存储 TOS 官方文档  
> 文档地址：https://www.volcengine.com/docs/6349/74837  
> 整理日期：2026-04-18
> API 实现原则：[API 实现六项原则](api_implementation_principles.md)

---

## 目录

1. [公共参数](#公共参数)
2. [CreateBucket - 创建桶](#1-createbucket---创建桶)
3. [HeadBucket - 查询桶元信息](#2-headbucket---查询桶元信息)
4. [DeleteBucket - 删除桶](#3-deletebucket---删除桶)
5. [ListBuckets - 列举桶](#4-listbuckets---列举桶)
6. [GetBucketLocation - 获取桶地域信息](#5-getbucketlocation---获取桶地域信息)

---

## 公共参数

所有 Bucket API 请求和响应都包含以下公共参数。

### 公共请求消息头

| 消息头名称 | 说明 |
|---|---|
| Authorization | 验证请求消息的合法性。关于 Authorization 的计算方法，请参见签名机制。 |
| Content-Length | RFC 2616 中定义的请求内容不包含消息头长度。 |
| Content-Type | 请求内容的类型。 |
| Date | 请求发起时的日期，GMT 时间，例如 `Fri, 30 Jul 2021 08:05:36 GMT`。 |
| Host | 域名。 |

### 公共响应消息头

| 消息头名称 | 说明 |
|---|---|
| Content-Length | 响应的消息体的长度。 |
| Date | 响应请求的日期，GMT 时间。 |
| Server | 响应请求的服务端。 |
| ETag | 在创建每个对象的时候生成，唯一标识一个对象的内容。 |
| x-tos-request-id | 此次请求的响应 ID，唯一标识这个请求。如果使用 TOS 服务遇到问题，可以凭借此字段联系 TOS 工作人员快速定位问题。 |
| x-tos-id-2 | 定位问题的特殊符号。 |
| x-tos-server-time | 请求在服务端的处理时间。 |

---

## 1. CreateBucket - 创建桶

> 文档地址: https://www.volcengine.com/docs/6349/74852

### 功能描述

创建一个新的 TOS 桶。要创建存储桶，您必须注册火山引擎账号，并拥有一个有效的 TOS AccessKey ID 来验证请求。不允许匿名请求创建桶，并且一个用户在全局内最多可创建 100 个桶。通过创建桶，您就成为桶的所有者。

### HTTP 方法与路径

```
PUT / HTTP/1.1
Host: <BucketName>.<Endpoint>
```

- **方法**: `PUT`
- **路径**: `/`
- **Host**: `<BucketName>.tos-<Region>.volces.com`（例如 `mybucket.tos-cn-beijing.volces.com`）

### 请求头

除公共请求消息头外，还支持以下请求头：

| 名称 | 位置 | 参数类型 | 是否必选 | 示例值 | 说明 |
|---|---|---|---|---|---|
| x-tos-acl | Header | String | 否 | private | 桶访问权限，有效值：`private`（私有，默认值）、`public-read`（公共读）、`public-read-write`（公共读写）、`authenticated-read`（认证用户读）、`bucket-owner-read`（桶所有者读）、`bucket-owner-full-control`（桶所有者完全权限）。 |
| x-tos-grant-full-control | Header | String | 否 | id="accountId" | 授予指定用户 FULL_CONTROL 权限。格式：`id="accountId"`。 |
| x-tos-grant-read | Header | String | 否 | id="accountId" | 授予指定用户 READ 权限。格式：`id="accountId"`。 |
| x-tos-grant-read-acp | Header | String | 否 | id="accountId" | 授予指定用户 READ_ACP 权限。格式：`id="accountId"`。 |
| x-tos-grant-write | Header | String | 否 | id="accountId" | 授予指定用户 WRITE 权限。格式：`id="accountId"`。 |
| x-tos-grant-write-acp | Header | String | 否 | id="accountId" | 授予指定用户 WRITE_ACP 权限。格式：`id="accountId"`。 |
| x-tos-storage-class | Header | String | 否 | STANDARD | 桶的默认存储类型。有效值：`STANDARD`（标准存储）、`IA`（低频访问存储）、`ARCHIVE_FR`（归档闪回存储）、`ARCHIVE`（归档存储）、`COLD_ARCHIVE`（冷归档存储）、`DEEP_COLD_ARCHIVE`（深度冷归档存储）。 |
| x-tos-az-redundancy | Header | String | 否 | single-az | 桶的可用区冗余类型。有效值：`single-az`（单 AZ 冗余，默认值）、`multi-az`（多 AZ 冗余）。 |
| x-tos-bucket-type | Header | String | 否 | fns | 桶类型。有效值：`fns`（扁平桶，默认值）、`hns`（分层桶，分层命名空间）。 |
| x-tos-project-name | Header | String | 否 | default | 桶需要创建在哪个 Project 下。如果未指定，则默认创建在 default 项目下。 |

### 请求体

该请求无请求消息体。

### 响应头

返回公共响应消息头。成功创建时还返回：

| 名称 | 参数类型 | 说明 |
|---|---|---|
| Location | String | 新创建桶的 URI。 |

### 响应体

该请求响应中无消息体。

### 请求示例

```http
PUT / HTTP/1.1
Host: examplebucket.tos-cn-beijing.volces.com
Date: Fri, 30 Jul 2021 08:05:36 GMT
x-tos-acl: private
x-tos-storage-class: STANDARD
x-tos-az-redundancy: single-az
Authorization: authorization string
```

### 响应示例

```http
HTTP/1.1 200 OK
Date: Fri, 30 Jul 2021 08:05:36 GMT
Server: TosServer
x-tos-id-2: d242440bbb9b000e-a444ed0
x-tos-request-id: d242440bbb9b000e-a444ed0
Location: /examplebucket
```

### 错误码

| HTTP 状态码 | 错误码 | 说明 |
|---|---|---|
| 400 | InvalidBucketName | 桶名称不符合命名规范。 |
| 403 | AccessDenied | 没有创建桶的权限或匿名请求。 |
| 409 | BucketAlreadyExists | 桶名已被其他用户占用。 |
| 409 | BucketAlreadyOwnedByYou | 同名桶已被当前用户拥有。 |
| 400 | TooManyBuckets | 已达到最大桶数量限制（全局最多 100 个）。 |
| 400 | InvalidStorageClass | 不支持的存储类型。 |
| 400 | InvalidAZRedundancy | 不支持的可用区冗余类型。 |

---

## 2. HeadBucket - 查询桶元信息

> 文档地址: https://www.volcengine.com/docs/6349/74851

### 功能描述

此接口用于判断桶是否存在和是否有桶的访问权限。

- 如果具有桶的访问权限并且桶存在，则返回 `200 OK` 状态码。
- 如果桶不存在或者没有访问桶的权限，此 HEAD 请求会返回 `404 Not Found` 或 `403 Forbidden` 状态码。

### HTTP 方法与路径

```
HEAD / HTTP/1.1
Host: <BucketName>.<Endpoint>
```

- **方法**: `HEAD`
- **路径**: `/`
- **Host**: `<BucketName>.tos-<Region>.volces.com`

### 请求头

使用公共请求消息头，无额外请求头。

### 请求体

该请求无请求消息体。

### 响应头

除公共响应消息头外，还返回以下响应头：

| 名称 | 参数类型 | 示例值 | 说明 |
|---|---|---|---|
| x-tos-bucket-region | String | cn-beijing | 桶所在地域。 |
| x-tos-storage-class | String | STANDARD | 桶的默认存储类型。取值：`STANDARD`（标准存储）、`IA`（低频访问存储）、`INTELLIGENT_TIERING`（智能分层存储）、`ARCHIVE_FR`（归档闪回存储）、`ARCHIVE`（归档存储）、`COLD_ARCHIVE`（冷归档存储）、`DEEP_COLD_ARCHIVE`（深度冷归档存储）。 |
| x-tos-project-name | String | default | 桶关联的项目名称。 |
| x-tos-az-redundancy | String | multi-az | 桶的可用区冗余类型。取值：`single-az`（单 AZ 冗余）、`multi-az`（多 AZ 冗余）。 |
| x-tos-bucket-type | String | hns | 桶的类型。取值：未返回此响应消息头表示扁平桶；`hns` 表示分层桶。 |

### 响应体

该请求响应中无消息体。

### 请求示例

```http
HEAD / HTTP/1.1
Host: bucketname.tos-cn-beijing.volces.com
Date: Fri, 30 Jul 2021 08:05:36 +0000
Authorization: authorization string
```

### 响应示例

```http
HTTP/1.1 200 OK
Date: Fri, 30 Jul 2021 08:05:36 GMT
Server: TosServer
x-tos-id-2: d242440bbb9b000e-a444ed0
x-tos-request-id: d242440bbb9b000e-a444ed0
x-tos-bucket-region: cn-beijing
x-tos-storage-class: STANDARD
x-tos-project-name: default
```

### 错误码

| HTTP 状态码 | 错误码 | 说明 |
|---|---|---|
| 403 | AccessDenied / Forbidden | 没有访问桶的权限。 |
| 404 | NoSuchBucket / Not Found | 桶不存在。 |

---

## 3. DeleteBucket - 删除桶

> 文档地址: https://www.volcengine.com/docs/6349/74849

### 功能描述

删除已经创建的桶。删除桶之前，要保证桶是空桶，即桶中的对象和分片数据已经被清除掉。

### HTTP 方法与路径

```
DELETE / HTTP/1.1
Host: <BucketName>.<Endpoint>
```

- **方法**: `DELETE`
- **路径**: `/`
- **Host**: `<BucketName>.tos-<Region>.volces.com`

### 请求头

使用公共请求消息头，无额外请求头。

### 请求体

该请求无请求消息体。

### 响应头

返回公共响应消息头。

### 响应体

该请求响应中无消息体。

### 请求示例

```http
DELETE / HTTP/1.1
Host: bucketName.tos-cn-beijing.volces.com
Date: Fri, 30 Jul 2021 06:45:39 GMT
Authorization: authorization string
```

### 响应示例

```http
HTTP/1.1 204 No Content
Content-Type: application/xml
Date: Fri, 30 Jul 2021 08:05:36 GMT
Server: TosServer
x-tos-id-2: 7a049d0bb80c000a-a444ed0
x-tos-request-id: 7a049d0bb80c000a-a444ed0
```

### 错误码

| HTTP 状态码 | 错误码 | 说明 |
|---|---|---|
| 403 | AccessDenied | 没有删除桶的权限。 |
| 404 | NoSuchBucket | 指定的桶不存在。 |
| 409 | BucketNotEmpty | 桶内仍有对象或分片数据，不能删除。 |

---

## 4. ListBuckets - 列举桶

> 文档地址: https://www.volcengine.com/docs/6349/74850

### 功能描述

此接口用于查询当前请求用户拥有的所有桶。

### HTTP 方法与路径

```
GET / HTTP/1.1
Host: tos-<Region>.volces.com
```

- **方法**: `GET`
- **路径**: `/`
- **Host**: `tos-<Region>.volces.com`（注意：ListBuckets 使用区域级域名，不包含桶名）

### 请求头

除公共请求消息头外，还支持以下请求头：

| 名称 | 位置 | 参数类型 | 是否必选 | 示例值 | 说明 |
|---|---|---|---|---|---|
| x-tos-project-name | Header | String | 否 | default | 通过此消息头列举指定项目名称下的桶。如果携带该消息头并指定 ProjectName，则 TOS 返回属于该项目下的所有桶。当指定的 ProjectName 为 `default` 时，TOS 返回属于默认项目下的所有桶。如果未携带该消息头，则 TOS 返回请求者拥有权限的所有桶。 |
| x-tos-bucket-type | Header | String | 否 | hns | 通过此消息头明确获取的列表内容。取值：`hns`（获取所有分层桶列表）、`fns`（获取所有扁平桶列表）。不带此消息头则获取所有桶列表。 |

### 请求体

该请求无请求消息体。

### 响应头

返回公共响应消息头。

### 响应体 (JSON)

| 名称 | 参数类型 | 示例值 | 说明 |
|---|---|---|---|
| Buckets | Array | - | 您拥有的桶列表信息。 |
| Buckets[].Name | String | bucketName | 桶名。 |
| Buckets[].CreationDate | String | 2021-08-19T09:16:05.000Z | 桶的创建时间（ISO 8601 格式）。 |
| Buckets[].Location | String | cn-beijing | 桶所在区域。 |
| Buckets[].ExtranetEndpoint | String | tos-cn-beijing.volces.com | 外部域名（公网访问域名）。 |
| Buckets[].IntranetEndpoint | String | tos-cn-beijing.ivolces.com | 内部域名（私网访问域名）。 |
| Buckets[].ProjectName | String | default | 桶关联的项目名称。 |
| Buckets[].BucketType | String | hns | 桶类型。`fns` 表示扁平桶，`hns` 表示分层桶。 |
| Owner | Object | - | 桶的所有者。 |
| Owner.ID | String | accountID | 账号 ID。 |

### 请求示例

```http
GET / HTTP/1.1
Host: tos-cn-beijing.volces.com
Date: Fri, 30 Jul 2021 13:53:55 +0000
Authorization: authorization string
```

### 响应示例

```http
HTTP/1.1 200 OK
Date: Fri, 30 Jul 2021 13:53:55 GMT
Server: TosServer
x-tos-id-2: 1e89f203jld00006-a444fd0
x-tos-request-id: 1e89f203b2d00006-a444ed0
Content-Length: 643

{
  "Buckets": [
    {
      "CreationDate": "2021-08-19T09:16:05.000Z",
      "Name": "buckettest001",
      "Location": "cn-beijing",
      "ExtranetEndpoint": "tos-cn-beijing.volces.com",
      "IntranetEndpoint": "tos-cn-beijing.ivolces.com",
      "ProjectName": "default"
    },
    {
      "CreationDate": "2021-05-06T02:27:04.000Z",
      "Name": "lynch-peking-bucket",
      "Location": "cn-beijing",
      "ExtranetEndpoint": "tos-cn-beijing.volces.com",
      "IntranetEndpoint": "tos-cn-beijing.ivolces.com",
      "ProjectName": "default"
    }
  ],
  "Owner": {
    "ID": "AccountID"
  }
}
```

### 错误码

| HTTP 状态码 | 错误码 | 说明 |
|---|---|---|
| 403 | AccessDenied | 没有列举桶的权限。 |

---

## 5. GetBucketLocation - 获取桶地域信息

> 文档地址: https://www.volcengine.com/docs/6349/764782

### 功能描述

此接口用于查询当前桶的地域信息。

### HTTP 方法与路径

```
GET /?location HTTP/1.1
Host: <BucketName>.<Endpoint>
```

- **方法**: `GET`
- **路径**: `/?location`
- **Host**: `<BucketName>.tos-<Region>.volces.com`

### 请求头

使用公共请求消息头，无额外请求头。

### 请求体

该请求无请求消息体。

### 响应头

返回公共响应消息头。

### 响应体 (JSON)

| 名称 | 参数类型 | 示例值 | 说明 |
|---|---|---|---|
| Region | String | cn-beijing | 桶的地域位置。 |
| ExtranetEndpoint | String | tos-cn-beijing.volces.com | 公网访问域名。 |
| IntranetEndpoint | String | tos-cn-beijing.ivolces.com | 私网访问域名。 |

### 请求示例

```http
GET /?location HTTP/1.1
Host: bucketname.tos-cn-beijing.volces.com
Date: Fri, 30 Jul 2021 13:53:55 GMT
Authorization: authorization string
```

### 响应示例

```http
HTTP/1.1 200 OK
Date: Fri, 30 Jul 2021 13:53:55 GMT
Server: TosServer
x-tos-id-2: 1e89f203jld00006-a444fd0
x-tos-request-id: 1e89f203b2d00006-a444ed0
Content-Length: 643

{
  "Region": "cn-beijing",
  "ExtranetEndpoint": "tos-cn-beijing.volces.com",
  "IntranetEndpoint": "tos-cn-beijing.ivolces.com"
}
```

### 错误码

| HTTP 状态码 | 错误码 | 说明 |
|---|---|---|
| 403 | AccessDenied | 没有访问桶的权限。 |
| 404 | NoSuchBucket | 指定的桶不存在。 |

---

## 附录：API 快速参照表

| API | HTTP 方法 | 路径 | Host 格式 | 请求体 | 响应体 |
|---|---|---|---|---|---|
| CreateBucket | PUT | `/` | `<Bucket>.tos-<Region>.volces.com` | 无 | 无 |
| HeadBucket | HEAD | `/` | `<Bucket>.tos-<Region>.volces.com` | 无 | 无 |
| DeleteBucket | DELETE | `/` | `<Bucket>.tos-<Region>.volces.com` | 无 | 无 |
| ListBuckets | GET | `/` | `tos-<Region>.volces.com` | 无 | JSON (桶列表) |
| GetBucketLocation | GET | `/?location` | `<Bucket>.tos-<Region>.volces.com` | 无 | JSON (地域信息) |

## 附录：TOS 地域与 Endpoint 对照

| 地域 | Region | 外网 Endpoint | 内网 Endpoint |
|---|---|---|---|
| 华北2（北京） | cn-beijing | tos-cn-beijing.volces.com | tos-cn-beijing.ivolces.com |
| 华东2（上海） | cn-shanghai | tos-cn-shanghai.volces.com | tos-cn-shanghai.ivolces.com |
| 华南1（广州） | cn-guangzhou | tos-cn-guangzhou.volces.com | tos-cn-guangzhou.ivolces.com |
| 亚太东南（柔佛） | ap-southeast-1 | tos-ap-southeast-1.volces.com | tos-ap-southeast-1.ivolces.com |

## 附录：存储类型说明

| 存储类型 | 值 | 说明 |
|---|---|---|
| 标准存储 | STANDARD | 频繁的数据访问，如社交图片、热点视频、大数据分析等。 |
| 低频访问存储 | IA | 不频繁访问（平均每月一到两次），如数据备份、文件同步等。 |
| 智能分层存储 | INTELLIGENT_TIERING | 自动根据访问频率在标准和低频之间转换。 |
| 归档闪回存储 | ARCHIVE_FR | 归档数据，需要快速恢复的场景。 |
| 归档存储 | ARCHIVE | 长期归档数据。 |
| 冷归档存储 | COLD_ARCHIVE | 极少访问的冷数据（部分地域支持）。 |
| 深度冷归档存储 | DEEP_COLD_ARCHIVE | 几乎不访问的数据，成本最低（部分地域支持，邀测状态）。 |

## 附录：桶 ACL 权限说明

| ACL 值 | 说明 |
|---|---|
| private | 私有，默认值。桶所有者拥有全部权限，其他用户无权限。 |
| public-read | 公共读。所有用户都可以读取桶中的对象。 |
| public-read-write | 公共读写。所有用户都可以读写桶中的对象。 |
| authenticated-read | 认证用户读。所有认证用户可以读取桶中的对象。 |
| bucket-owner-read | 桶所有者读。 |
| bucket-owner-full-control | 桶所有者完全权限。 |
