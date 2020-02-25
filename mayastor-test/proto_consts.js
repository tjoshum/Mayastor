'use strict';

//TODO: derive this by reading the mayastor.proto
const ShareProtocol = {
    NONE: 0,
    NVMF: 1,
    ISCSI: 2,
    NBD: 3,
};

module.exports = {
    ShareProtocol,
}
